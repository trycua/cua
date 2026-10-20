//! `get_guide` through the real registry: listed, callable without a session,
//! closed to unknown arguments, and pointed at from the tools that need it.

use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

struct Stub(ToolDef);

#[async_trait::async_trait]
impl Tool for Stub {
    fn def(&self) -> &ToolDef {
        &self.0
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::text("stub")
    }
}

fn stub(name: &str, description: &str) -> Box<dyn Tool> {
    Box::new(Stub(ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": {}}),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    }))
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_session_tools();
    registry
}

fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn entry(list: &Value, name: &str) -> Value {
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed"))
        .clone()
}

#[test]
fn get_guide_is_listed_as_a_read_only_tool_that_names_its_topics() {
    let list = registry().tools_list();
    let tool = entry(&list, "get_guide");
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["annotations"]["destructiveHint"], false);
    assert_eq!(tool["annotations"]["openWorldHint"], false);
    let description = tool["description"].as_str().unwrap();
    for topic in ["drag", "browser", "permissions", "batching", "menus"] {
        assert!(description.contains(topic), "{description}");
    }
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert!(tool["inputSchema"]["properties"]["topic"].is_object());
}

#[tokio::test]
async fn an_agent_with_no_skill_installed_can_fetch_a_guide_by_tool_call() {
    let registry = registry();
    for topic in ["drag", "browser", "permissions", "batching", "menus"] {
        let result = registry.invoke("get_guide", json!({"topic": topic})).await;
        assert_ne!(result.is_error, Some(true), "{topic}: {result:?}");
        let text = text_of(&result);
        assert!(
            text.starts_with(&format!("Cua Driver guide: {topic}")),
            "{text}"
        );
        assert!(text.len() > 500, "{topic} returned almost nothing");
        assert_eq!(result.structured_content.unwrap()["topic"], topic);
    }
}

#[tokio::test]
async fn an_unknown_topic_is_an_error_naming_the_valid_topics() {
    let result = registry()
        .invoke("get_guide", json!({"topic": "teleportation"}))
        .await;
    assert_eq!(result.is_error, Some(true));
    let message = text_of(&result);
    assert!(message.contains("teleportation"), "{message}");
    for topic in ["drag", "browser", "permissions", "batching", "menus"] {
        assert!(message.contains(topic), "{message}");
    }
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "unknown_guide_topic"
    );
}

#[tokio::test]
async fn no_topic_lists_the_topics_and_unknown_arguments_are_refused() {
    let registry = registry();
    let listed = registry.invoke("get_guide", json!({})).await;
    assert_ne!(listed.is_error, Some(true));
    assert!(text_of(&listed).contains("- drag:"));
    let refused = registry
        .invoke("get_guide", json!({"topic": "drag", "section": "x"}))
        .await;
    assert_eq!(refused.is_error, Some(true), "{refused:?}");
}

#[test]
fn tools_that_most_need_a_guide_point_at_it() {
    let mut registry = registry();
    for tool in [
        "drag",
        "get_browser_state",
        "browser_prepare",
        "check_permissions",
        "invoke_menu",
    ] {
        registry.register(stub(tool, "Does the thing."));
    }
    registry.register(stub("click", "Clicks."));
    let list = registry.tools_list();
    let described = |name: &str| {
        entry(&list, name)["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (tool, topic) in [
        ("drag", "drag"),
        ("get_browser_state", "browser"),
        ("browser_prepare", "browser"),
        ("check_permissions", "permissions"),
        ("invoke_menu", "menus"),
        ("run_steps", "batching"),
    ] {
        assert!(
            described(tool).contains(&format!("get_guide({{topic:\"{topic}\"}})")),
            "{tool}: {}",
            described(tool)
        );
    }
    assert_eq!(described("click"), "Clicks.");
    assert!(described("drag").starts_with("Does the thing. "));
}
