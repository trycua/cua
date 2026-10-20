//! `get_guide` over the real MCP wire: the server instructions and tool
//! descriptions point at it, and an agent with no skill installed can fetch a
//! guide, or learn the valid topics from an error, through a tool call.

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

const TOPICS: [&str; 5] = ["drag", "browser", "permissions", "batching", "menus"];

fn spawn() -> Option<RawDriver> {
    let mut driver = RawDriver::spawn()?;
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "get-guide-test", "version": "1" }
        }
    }));
    let init = driver.recv();
    let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("`get_guide({topic})`"),
        "the instructions must point at get_guide: {instructions}"
    );
    assert!(
        instructions.split_whitespace().count() <= 200,
        "the instructions keep their 200-word cap"
    );
    for topic in TOPICS {
        assert!(instructions.contains(topic), "{topic}: {instructions}");
    }
    assert!(instructions.contains("skill://cua-driver/SKILL.md"));
    Some(driver)
}

fn call(driver: &mut RawDriver, arguments: Value) -> Value {
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "get_guide", "arguments": arguments }
    }));
    driver.recv()["result"].clone()
}

fn text(result: &Value) -> String {
    result["content"]
        .as_array()
        .expect("content array")
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tools_list_has_get_guide_and_the_tools_that_need_it_point_at_it() {
    let Some(mut driver) = spawn() else { return };
    driver.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let listed = driver.recv();
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let describe = |name: &str| -> Option<String> {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| tool["description"].as_str().unwrap_or_default().to_owned())
    };
    let guide = tools
        .iter()
        .find(|tool| tool["name"] == "get_guide")
        .expect("get_guide is listed");
    assert_eq!(guide["annotations"]["readOnlyHint"], true);
    for topic in TOPICS {
        assert!(guide["description"].as_str().unwrap().contains(topic));
    }
    for (tool, topic) in [
        ("drag", "drag"),
        ("check_permissions", "permissions"),
        ("get_browser_state", "browser"),
        ("browser_prepare", "browser"),
        ("run_steps", "batching"),
        ("invoke_menu", "menus"),
    ] {
        // A tool a platform does not register has nothing to point from.
        if let Some(description) = describe(tool) {
            assert!(
                description.contains(&format!("get_guide({{topic:\"{topic}\"}})")),
                "{tool}: {description}"
            );
        }
    }
}

#[test]
fn an_agent_with_no_skill_installed_can_fetch_the_drag_and_browser_guidance() {
    let Some(mut driver) = spawn() else { return };
    for topic in TOPICS {
        let result = call(&mut driver, json!({ "topic": topic }));
        assert_ne!(result["isError"].as_bool(), Some(true), "{topic}: {result}");
        let body = text(&result);
        assert!(
            body.starts_with(&format!("Cua Driver guide: {topic}")),
            "{body}"
        );
        assert!(body.len() > 500, "{topic} returned almost nothing");
        assert_eq!(result["structuredContent"]["topic"], topic);
    }
    let drag = text(&call(&mut driver, json!({"topic": "drag"})));
    assert!(drag.contains("`drag`"), "{drag}");
    let browser = text(&call(&mut driver, json!({"topic": "browser"})));
    assert!(browser.contains("browser_prepare"), "{browser}");
}

#[test]
fn an_unknown_topic_is_an_error_that_names_the_valid_topics() {
    let Some(mut driver) = spawn() else { return };
    let result = call(&mut driver, json!({"topic": "juggling"}));
    assert_eq!(result["isError"].as_bool(), Some(true), "{result}");
    let message = text(&result);
    assert!(message.contains("juggling"), "{message}");
    for topic in TOPICS {
        assert!(message.contains(topic), "{message}");
    }
    assert_eq!(result["structuredContent"]["code"], "unknown_guide_topic");

    let listed = call(&mut driver, json!({}));
    assert_ne!(listed["isError"].as_bool(), Some(true));
    assert!(text(&listed).contains("- menus:"));
}
