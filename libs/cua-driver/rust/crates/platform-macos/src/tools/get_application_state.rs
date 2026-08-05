use async_trait::async_trait;
use cua_driver_core::{
    application_observation,
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
    tool_args::ArgsExt,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub struct GetApplicationStateTool;

struct ReadOnlyTreeWalk(crate::ax::tree::TreeWalkResult);

impl Drop for ReadOnlyTreeWalk {
    fn drop(&mut self) {
        crate::ax::tree::release_actionable_nodes(&self.0.nodes);
    }
}

#[async_trait]
impl Tool for GetApplicationStateTool {
    fn def(&self) -> &ToolDef {
        application_observation::tool_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.require_i32("pid") {
            Ok(pid) if pid > 0 => pid,
            Ok(_) => return ToolResult::error("pid must be a positive integer"),
            Err(error) => return error,
        };
        if args.get("scope").and_then(Value::as_str) != Some("application_composite") {
            return ToolResult::error(
                "scope must be the explicit value application_composite; exact-window failures never select it implicitly",
            )
            .with_structured(json!({
                "code": "invalid_observation_scope",
                "effect": "refused",
                "pid": pid,
            }));
        }
        if !crate::permissions::status::accessibility_granted() {
            return ToolResult::error(
                "get_application_state requires macOS Accessibility permission",
            )
            .with_structured(json!({
                "code": "capability_unavailable",
                "effect": "refused",
                "pid": pid,
                "scope": { "kind": "application_composite" },
                "capability": application_observation::capability("permission_denied", "macos"),
            }));
        }

        let query = args.get("query").and_then(Value::as_str).map(str::to_owned);
        let max_elements = bounded_arg(&args, "max_elements", 2_000, 2_000);
        let max_depth = bounded_arg(&args, "max_depth", 25, 25);
        let walk = tokio::task::spawn_blocking(move || {
            ReadOnlyTreeWalk(crate::ax::tree::walk_tree_bounded(
                pid,
                None,
                None,
                max_elements,
                max_depth,
            ))
        });
        let result = match tokio::time::timeout(std::time::Duration::from_secs(20), walk).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                return ToolResult::error(format!(
                    "application-composite AX walk failed for pid={pid}: {error}"
                ))
            }
            Err(_) => {
                return ToolResult::error(format!(
                    "application-composite AX walk for pid={pid} timed out after 20 s"
                ))
            }
        };

        let all_elements = observation_elements(&result.0.nodes, pid);
        let element_count = all_elements.len();
        let elements = project_elements(all_elements, query.as_deref());
        let tree_markdown = render_observation_tree(&elements);
        let scope_id = format!("appax:{}", uuid::Uuid::new_v4());
        let mut structured = json!({
            "pid": pid,
            "scope": application_observation::scope_identity(pid, &scope_id),
            "capability": application_observation::capability("supported", "macos"),
            "element_count": element_count,
            "returned_element_count": elements.len(),
            "elements_complete": false,
            "truncated": result.0.truncated,
            "tree_markdown": tree_markdown,
            "elements": elements,
            "actions_supported": false,
            "screenshot_supported": false,
            "_note": "observation_index is read-only application-composite identity. It is not an element_index or element_token and cannot be passed to action tools."
        });
        if query.is_some() {
            structured["query_applied"] = Value::Bool(true);
        }
        if structured["element_count"] == 0 {
            structured["degraded"] = Value::Bool(true);
            structured["degraded_reason"] = Value::String(
                "ax_tree_empty: the application AX root returned no actionable semantic elements"
                    .into(),
            );
        }

        ToolResult {
            content: vec![Content::text(format!(
                "pid={pid} scope=application_composite elements={}\n\n{}",
                structured["returned_element_count"], tree_markdown
            ))],
            is_error: None,
            structured_content: Some(structured),
            action_record: None,
        }
    }
}

fn bounded_arg(args: &Value, name: &str, default: usize, maximum: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, maximum as u64) as usize)
        .unwrap_or(default)
}

fn observation_elements(nodes: &[crate::ax::tree::AXNode], pid: i32) -> Vec<Value> {
    observation_elements_with_owner(nodes, |node| element_belongs_to_pid(node, pid))
}

fn element_belongs_to_pid(node: &crate::ax::tree::AXNode, pid: i32) -> bool {
    if node.element_ptr == 0 {
        return false;
    }
    let mut owner_pid = 0;
    let status = unsafe {
        crate::ax::bindings::AXUIElementGetPid(
            node.element_ptr as crate::ax::bindings::AXUIElementRef,
            &mut owner_pid,
        )
    };
    status == crate::ax::bindings::kAXErrorSuccess && owner_pid == pid
}

fn observation_elements_with_owner(
    nodes: &[crate::ax::tree::AXNode],
    belongs_to_pid: impl Fn(&crate::ax::tree::AXNode) -> bool,
) -> Vec<Value> {
    nodes
        .iter()
        .filter_map(|node| {
            if !belongs_to_pid(node) {
                return None;
            }
            let observation_index = node.element_index?;
            let label = node
                .title
                .clone()
                .or_else(|| node.description.clone())
                .or_else(|| node.value.clone())
                .or_else(|| node.identifier.clone());
            let mut entry = json!({
                "observation_index": observation_index,
                "role": node.role,
                "depth": node.depth,
            });
            if let Some(label) = label {
                entry["label"] = Value::String(label);
            }
            if let Some(value) = node.value_state.clone().or_else(|| node.value.clone()) {
                entry["value"] = Value::String(value);
            }
            if let Some(parent) = node.parent_element_index {
                entry["parent_observation_index"] = json!(parent);
            }
            if let Some([x, y, width, height]) = node.frame {
                entry["frame"] = json!({"x": x, "y": y, "w": width, "h": height});
            }
            if let Some(enabled) = node.enabled {
                entry["enabled"] = Value::Bool(enabled);
            }
            if let Some(selected) = node.selected {
                entry["selected"] = Value::Bool(selected);
            }
            Some(entry)
        })
        .collect()
}

fn project_elements(elements: Vec<Value>, query: Option<&str>) -> Vec<Value> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return elements;
    };
    let needle = query.to_ascii_lowercase();
    let parents: BTreeMap<u64, u64> = elements
        .iter()
        .filter_map(|element| {
            Some((
                element.get("observation_index")?.as_u64()?,
                element.get("parent_observation_index")?.as_u64()?,
            ))
        })
        .collect();
    let mut selected = BTreeSet::new();
    for element in &elements {
        let search_text = ["role", "label", "value"]
            .into_iter()
            .filter_map(|field| element.get(field).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if search_text.contains(&needle) {
            let Some(mut index) = element.get("observation_index").and_then(Value::as_u64) else {
                continue;
            };
            selected.insert(index);
            while let Some(parent) = parents.get(&index).copied() {
                selected.insert(parent);
                index = parent;
            }
        }
    }
    elements
        .into_iter()
        .filter(|element| {
            element
                .get("observation_index")
                .and_then(Value::as_u64)
                .is_some_and(|index| selected.contains(&index))
        })
        .collect()
}

fn render_observation_tree(elements: &[Value]) -> String {
    let mut output = String::new();
    for element in elements {
        let depth = element.get("depth").and_then(Value::as_u64).unwrap_or(0);
        let index = element
            .get("observation_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let role = element
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let label = element.get("label").and_then(Value::as_str).unwrap_or("");
        output.push_str(&"  ".repeat(depth as usize));
        output.push_str(&format!("- [observation {index}] {role}"));
        if !label.is_empty() {
            output.push_str(&format!(" \"{label}\""));
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        index: usize,
        parent: Option<usize>,
        role: &str,
        title: &str,
    ) -> crate::ax::tree::AXNode {
        crate::ax::tree::AXNode {
            element_index: Some(index),
            role: role.into(),
            title: Some(title.into()),
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: vec!["AXPress".into()],
            element_ptr: 0,
            depth: usize::from(parent.is_some()),
            parent_element_index: parent,
            frame: None,
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: Some(true),
            selected: None,
            in_web_content: false,
        }
    }

    #[test]
    fn application_observations_do_not_expose_action_handles() {
        let elements = observation_elements_with_owner(
            &[
                node(0, None, "AXWindow", "doc"),
                node(1, Some(0), "AXButton", "save"),
            ],
            |_| true,
        );
        assert_eq!(elements[1]["observation_index"], 1);
        assert_eq!(elements[1]["parent_observation_index"], 0);
        for element in elements {
            assert!(element.get("element_index").is_none());
            assert!(element.get("element_token").is_none());
            assert!(element.get("actions").is_none());
        }
    }

    #[test]
    fn query_keeps_matching_observation_and_its_ancestors() {
        let elements = observation_elements_with_owner(
            &[
                node(0, None, "AXWindow", "doc"),
                node(1, Some(0), "AXButton", "save"),
                node(2, Some(0), "AXButton", "cancel"),
            ],
            |_| true,
        );
        let projected = project_elements(elements, Some("save"));
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0]["observation_index"], 0);
        assert_eq!(projected[1]["observation_index"], 1);
    }

    #[test]
    fn rendered_tree_names_observations_not_elements() {
        let rendered = render_observation_tree(&observation_elements_with_owner(
            &[node(0, None, "AXButton", "save")],
            |_| true,
        ));
        assert!(rendered.contains("[observation 0]"));
        assert!(!rendered.contains("element_index"));
    }

    #[test]
    fn foreign_process_nodes_are_omitted() {
        let elements = observation_elements_with_owner(
            &[
                node(0, None, "AXButton", "same process"),
                node(1, None, "AXButton", "foreign process"),
            ],
            |node| node.element_index == Some(0),
        );
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["label"], "same process");
    }
}
