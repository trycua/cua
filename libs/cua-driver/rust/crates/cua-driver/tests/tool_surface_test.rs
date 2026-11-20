// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The default MCP tool surface: the `agent` profile with compact docs.
//!
//! An agent harness that loads every listed schema on every request pays for
//! this surface each time, so it has a size budget. Nothing is removed: hidden
//! tools stay callable by name, and the full docs of any tool are one
//! `get_guide({tool})` call away.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use cua_driver_core::tool_profile::{ToolProfile, AGENT_GROUP, GROUPS};
use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

/// Budget for what a model sees of the default list: the sum over listed tools
/// of name + description + the JSON of the input schema.
const VISIBLE_BUDGET_BYTES: usize = 60 * 1024;

fn initialize(driver: &mut RawDriver) -> Value {
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "tool-surface-test", "version": "1" }
        }
    }));
    driver.recv()["result"].clone()
}

fn tools_list(driver: &mut RawDriver) -> Value {
    driver.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    driver.recv()["result"].clone()
}

fn call(driver: &mut RawDriver, name: &str, arguments: Value) -> Value {
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    }));
    driver.recv()["result"].clone()
}

fn names(list: &Value) -> Vec<String> {
    list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect()
}

fn visible_bytes(list: &Value) -> usize {
    list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| {
            tool["name"].as_str().unwrap_or_default().len()
                + tool["description"].as_str().unwrap_or_default().len()
                + tool["inputSchema"].to_string().len()
        })
        .sum()
}

#[test]
fn every_registered_tool_belongs_to_a_group() {
    // The full surface, as contract tests see it.
    let Some(mut driver) = RawDriver::spawn() else {
        return;
    };
    initialize(&mut driver);
    let list = tools_list(&mut driver);
    for name in names(&list) {
        assert!(
            ToolProfile::group_of(&name).is_some(),
            "{name} is registered but in no tool group: add it to tool_profile::GROUPS \
             (a tool in no group is listed by default, which is rarely what you want)"
        );
    }
}

#[test]
fn the_default_list_is_the_agent_profile_within_its_size_budget() {
    let Some(mut driver) = RawDriver::spawn_default_surface() else {
        return;
    };
    let init = initialize(&mut driver);
    let list = tools_list(&mut driver);
    let listed = names(&list);

    for name in &listed {
        assert_eq!(
            ToolProfile::group_of(name),
            Some(AGENT_GROUP),
            "{name} is listed by default but is not in the agent group"
        );
    }
    #[cfg(target_os = "macos")]
    {
        let agent = GROUPS
            .iter()
            .find(|(group, _)| *group == AGENT_GROUP)
            .unwrap()
            .1;
        for name in agent {
            assert!(
                listed.iter().any(|l| l == name),
                "{name} missing from the default list"
            );
        }
        assert_eq!(listed.len(), 34, "{listed:?}");
    }

    let visible = visible_bytes(&list);
    eprintln!(
        "default tools/list: {} tools, {visible} bytes visible to the model (budget {VISIBLE_BUDGET_BYTES})",
        listed.len()
    );
    assert!(visible <= VISIBLE_BUDGET_BYTES, "{visible} bytes");

    // What was hidden is reported, and the instructions say how to get it.
    let hidden = &list["tool_profile"]["hidden"];
    assert!(hidden.is_object(), "{}", list["tool_profile"]);
    let instructions = init["instructions"].as_str().expect("instructions");
    assert!(
        instructions.contains("CUA_DRIVER_TOOL_PROFILE=full"),
        "{instructions}"
    );
    assert!(instructions.contains("get_guide({tool})"), "{instructions}");

    // The instructions never name a tool the list hides.
    for (_, tools) in hidden.as_object().unwrap() {
        for tool in tools.as_array().unwrap() {
            let tool = tool.as_str().unwrap();
            assert!(
                !instructions.contains(&format!("`{tool}`")),
                "the instructions name {tool}, which the default list hides: {instructions}"
            );
        }
    }
}

#[test]
fn compact_docs_keep_required_parameters_and_hide_advanced_ones() {
    let Some(mut driver) = RawDriver::spawn_default_surface() else {
        return;
    };
    initialize(&mut driver);
    let list = tools_list(&mut driver);
    for tool in list["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let properties = schema["properties"].as_object();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for key in &required {
            assert!(
                properties.is_some_and(|p| p.contains_key(*key)),
                "{name}: required parameter {key} is not listed"
            );
        }
        for advanced in ["session", "target", "debug_image_out"] {
            if !required.contains(&advanced) {
                assert!(
                    properties.is_none_or(|p| !p.contains_key(advanced)),
                    "{name}: {advanced} should not be listed"
                );
            }
        }
        let description = tool["description"].as_str().unwrap_or_default();
        assert!(
            !description.contains("\n\n"),
            "{name}: not a lead paragraph"
        );
        assert!(description.chars().count() <= 700, "{name}: {description}");
    }
}

#[test]
fn hidden_tools_stay_callable_by_name() {
    let Some(mut driver) = RawDriver::spawn_default_surface() else {
        return;
    };
    initialize(&mut driver);
    let list = tools_list(&mut driver);
    assert!(
        !names(&list).iter().any(|name| name == "get_config"),
        "get_config is in the admin group and hidden by default"
    );
    let result = call(&mut driver, "get_config", json!({}));
    assert_ne!(result["isError"].as_bool(), Some(true), "{result}");
    assert!(
        !result.to_string().contains("Unknown tool"),
        "a hidden tool must stay callable: {result}"
    );
}

#[test]
fn get_guide_returns_the_full_docs_of_a_tool() {
    let Some(mut driver) = RawDriver::spawn_default_surface() else {
        return;
    };
    initialize(&mut driver);
    let list = tools_list(&mut driver);
    let listed_click = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "click")
        .expect("click is listed");
    let compact_len = listed_click["description"].as_str().unwrap().len()
        + listed_click["inputSchema"].to_string().len();

    let result = call(&mut driver, "get_guide", json!({"tool": "click"}));
    assert_ne!(result["isError"].as_bool(), Some(true), "{result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["tool"], "click");
    let full_len = structured["description"].as_str().unwrap().len()
        + structured["inputSchema"].to_string().len();
    assert!(
        full_len > compact_len * 2,
        "full docs ({full_len}) should be well over the compact docs ({compact_len})"
    );
    // Advanced parameters the compact schema leaves out are documented here.
    assert!(
        structured["inputSchema"]["properties"]["session"].is_object(),
        "{structured}"
    );

    // A hidden tool has docs too; an unknown name says so.
    let hidden = call(&mut driver, "get_guide", json!({"tool": "get_config"}));
    assert_ne!(hidden["isError"].as_bool(), Some(true), "{hidden}");
    let unknown = call(&mut driver, "get_guide", json!({"tool": "no_such_tool"}));
    assert_eq!(unknown["isError"].as_bool(), Some(true), "{unknown}");
    assert_eq!(unknown["structuredContent"]["code"], "unknown_tool");
}
