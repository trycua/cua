// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The optional policy head must be invisible until it is configured.
//!
//! `suggest_action` sends a projection of a window's accessibility tree to a
//! third-party model, so a driver that was never given a credential must
//! behave exactly as it did before the feature existed: the tool absent from
//! `tools/list`, a call to it refused, and every other tool's advertised
//! entry unchanged. These tests assert that against a spawned driver rather
//! than against the registry in-process, so platform registration drift
//! cannot pass a core-only test.
//!
//! An empty `TYPESAFE_API_KEY` is the deterministic way to say "not
//! configured" here: the provider trims and rejects a blank credential, and
//! passing it explicitly means the result does not depend on whether the
//! developer's own shell exports a real key.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

const TOOL: &str = "suggest_action";
const KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Kept unreachable on purpose: every test here reads `tools/list` or an
/// unconfigured refusal, so none of them should ever open a socket. If one
/// starts to, it fails fast instead of reaching the real endpoint.
const UNREACHABLE_ENDPOINT: &str = "http://127.0.0.1:1/v1/systemone";

/// `tools/list` from a driver spawned with the given environment.
fn roster(env: &[(&str, &str)]) -> Option<Vec<Value>> {
    let mut driver = RawDriver::spawn_with_env(env)?;
    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    driver.recv();
    driver.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let response = driver.recv();
    Some(
        response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .clone(),
    )
}

fn find<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
    tools.iter().find(|tool| tool["name"] == name)
}

#[test]
fn an_unconfigured_driver_advertises_the_same_roster_it_always_did() {
    let Some(unconfigured) = roster(&[(KEY_ENV, "")]) else {
        return;
    };
    assert!(
        find(&unconfigured, TOOL).is_none(),
        "{TOOL} must not be advertised without a credential"
    );

    let Some(configured) = roster(&[
        (KEY_ENV, "test-credential"),
        ("TYPESAFE_BASE_URL", UNREACHABLE_ENDPOINT),
    ]) else {
        return;
    };
    assert!(
        find(&configured, TOOL).is_some(),
        "{TOOL} must be advertised once a credential is configured"
    );

    // The decisive assertion: configuring the policy head adds exactly one
    // entry and rewrites none. Comparing whole entries covers the schema,
    // the description, the annotations, and the risk metadata at once, so a
    // future change that quietly alters another tool cannot pass here.
    let mut added: Vec<&Value> = configured
        .iter()
        .filter(|tool| !unconfigured.contains(tool))
        .collect();
    assert_eq!(
        added.len(),
        1,
        "configuring the policy head changed more than one tool entry: {:?}",
        added
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert_eq!(added.remove(0)["name"], TOOL);

    let removed: Vec<&str> = unconfigured
        .iter()
        .filter(|tool| !configured.contains(tool))
        .map(|tool| tool["name"].as_str().unwrap_or("?"))
        .collect();
    assert!(
        removed.is_empty(),
        "configuring the policy head changed or removed existing tools: {removed:?}"
    );
}

#[test]
fn calling_the_policy_head_unconfigured_is_refused_without_reaching_a_provider() {
    let Some(mut driver) = RawDriver::spawn_with_env(&[(KEY_ENV, "")]) else {
        return;
    };
    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    driver.recv();
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": TOOL, "arguments": { "goal": "anything", "pid": 1, "window_id": 1 } }
    }));
    let response = driver.recv();

    // An unregistered tool must fail, and it must fail as "unknown tool"
    // rather than as a provider error — the credential check happens before
    // registration, so nothing about TypeSafe is reachable from here.
    let failed = response.get("error").is_some()
        || response["result"]["isError"] == json!(true)
        || response["result"]["structuredContent"]["refusal"].is_object();
    assert!(
        failed,
        "calling an unregistered {TOOL} must fail, got: {response}"
    );
    let rendered = response.to_string();
    assert!(
        !rendered.contains("typesafe.ai"),
        "an unconfigured refusal must not mention a provider endpoint: {rendered}"
    );
}

#[test]
fn the_advertised_schema_matches_the_house_style() {
    let Some(tools) = roster(&[
        (KEY_ENV, "test-credential"),
        ("TYPESAFE_BASE_URL", UNREACHABLE_ENDPOINT),
    ]) else {
        return;
    };
    let Some(tool) = find(&tools, TOOL) else {
        panic!("{TOOL} not advertised with a credential configured");
    };
    let schema = &tool["inputSchema"];

    assert_eq!(schema["type"], "object", "{TOOL} must be a plain object");
    for unsupported in ["anyOf", "oneOf", "allOf"] {
        assert!(
            schema.get(unsupported).is_none(),
            "{TOOL} top-level {unsupported} is rejected by Bedrock: {schema}"
        );
    }
    assert_eq!(
        schema["additionalProperties"], false,
        "{TOOL} must reject unknown arguments like every other tool"
    );
    assert_eq!(
        schema["required"],
        json!(["goal", "pid", "window_id"]),
        "{TOOL} required set drifted"
    );
    for field in [
        "session",
        "goal",
        "pid",
        "window_id",
        "deny",
        "history",
        "max_elements",
    ] {
        assert!(
            schema["properties"][field].is_object(),
            "{TOOL} schema missing {field}"
        );
    }
    assert!(
        schema["properties"]["session"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("session label")),
        "{TOOL} must carry the canonical multi-call session guidance"
    );
    assert_eq!(
        schema["properties"]["max_elements"]["maximum"],
        json!(120),
        "{TOOL} must advertise its element ceiling so a client cannot ask for more"
    );

    assert_eq!(
        tool["annotations"]["readOnlyHint"], true,
        "{TOOL} advises and never dispatches input"
    );
    assert_eq!(
        tool["annotations"]["destructiveHint"], false,
        "{TOOL} must not be advertised as destructive"
    );
    assert_eq!(
        tool["annotations"]["openWorldHint"], true,
        "{TOOL} calls a third-party service"
    );
    assert_eq!(
        tool["risk"]["class"], "r3",
        "{TOOL} egresses a tree projection and must stay in the external-effect tier"
    );

    // The description has to tell a caller the two things it cannot discover
    // from the schema: that the tool never writes text, and that `deny` is
    // enforced rather than suggested.
    let description = tool["description"].as_str().expect("description");
    for promise in ["never generates text", "enforced in code"] {
        assert!(
            description.contains(promise),
            "{TOOL} description must state {promise:?}"
        );
    }
}
