use async_trait::async_trait;
use cua_driver_core::{
    accessibility_surface,
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
    tool_args::ArgsExt,
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

use super::ToolState;

pub struct ListAccessibilitySurfacesTool {
    state: Arc<ToolState>,
}

pub struct GetAccessibilitySurfaceStateTool {
    state: Arc<ToolState>,
}

impl ListAccessibilitySurfacesTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

impl GetAccessibilitySurfaceStateTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

fn require_pid(args: &Value) -> Result<i32, ToolResult> {
    match args.require_i32("pid") {
        Ok(pid) if pid > 0 => Ok(pid),
        Ok(_) => Err(ToolResult::error("pid must be a positive integer")
            .with_structured(json!({"code": "invalid_pid", "effect": "refused"}))),
        Err(error) => Err(error),
    }
}

fn permission_refusal(pid: i32, capability_name: &str) -> ToolResult {
    ToolResult::error("accessibility-only surfaces require macOS Accessibility permission")
        .with_structured(json!({
            "code": "capability_unavailable",
            "effect": "refused",
            "pid": pid,
            "capability": accessibility_surface::capability(
                capability_name,
                "permission_denied",
                "macos",
            ),
        }))
}

#[async_trait]
impl Tool for ListAccessibilitySurfacesTool {
    fn def(&self) -> &ToolDef {
        accessibility_surface::list_tool_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match require_pid(&args) {
            Ok(pid) => pid,
            Err(error) => return error,
        };
        if !crate::permissions::status::accessibility_granted() {
            return permission_refusal(pid, accessibility_surface::DISCOVERY_CAPABILITY);
        }

        let registry = Arc::clone(&self.state.accessibility_surfaces);
        let surfaces = match tokio::task::spawn_blocking(move || registry.discover(pid)).await {
            Ok(surfaces) => surfaces,
            Err(error) => {
                return ToolResult::error(format!(
                    "accessibility surface discovery failed for pid={pid}: {error}"
                ))
            }
        };
        ToolResult::text(format!(
            "Found {} accessibility-only window surface(s) for pid={pid}.",
            surfaces.len()
        ))
        .with_structured(json!({
            "pid": pid,
            "surfaces": surfaces,
            "capability": accessibility_surface::capability(
                accessibility_surface::DISCOVERY_CAPABILITY,
                "supported",
                "macos",
            ),
        }))
    }
}

#[async_trait]
impl Tool for GetAccessibilitySurfaceStateTool {
    fn def(&self) -> &ToolDef {
        accessibility_surface::get_tool_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match require_pid(&args) {
            Ok(pid) => pid,
            Err(error) => return error,
        };
        let surface_token = match args.require_str("surface_token") {
            Ok(token) => token,
            Err(error) => return error,
        };
        if !crate::permissions::status::accessibility_granted() {
            return permission_refusal(pid, accessibility_surface::OBSERVATION_CAPABILITY);
        }
        let query = args.get("query").and_then(Value::as_str).map(str::to_owned);
        let max_elements = bounded_arg(&args, "max_elements", 2_000, 2_000);
        let max_depth = bounded_arg(&args, "max_depth", 25, 25);
        let registry = Arc::clone(&self.state.accessibility_surfaces);
        let token_for_walk = surface_token.clone();
        let observation = match tokio::task::spawn_blocking(move || {
            let surface = registry.resolve(pid, &token_for_walk)?;
            Ok::<_, crate::ax::surface::ResolveError>(crate::ax::surface::observe(
                &surface,
                pid,
                max_elements,
                max_depth,
            ))
        })
        .await
        {
            Ok(Ok(observation)) => observation,
            Ok(Err(error)) => return surface_refusal(pid, &surface_token, error),
            Err(error) => {
                return ToolResult::error(format!(
                    "accessibility surface observation failed for pid={pid}: {error}"
                ))
            }
        };

        let total_node_count = observation.nodes.len();
        let nodes = project_nodes(observation.nodes, query.as_deref());
        let tree_markdown = render_tree(&nodes);
        ToolResult {
            content: vec![Content::text(format!(
                "pid={pid} surface=ax_window nodes={}\n\n{tree_markdown}",
                nodes.len()
            ))],
            is_error: None,
            structured_content: Some(json!({
                "pid": pid,
                "surface": {
                    "kind": accessibility_surface::SURFACE_KIND,
                    "surface_token": surface_token,
                },
                "capability": accessibility_surface::capability(
                    accessibility_surface::OBSERVATION_CAPABILITY,
                    "supported",
                    "macos",
                ),
                "node_count": total_node_count,
                "returned_node_count": nodes.len(),
                "truncated": observation.truncated,
                "query_applied": query.is_some(),
                "tree_markdown": tree_markdown,
                "nodes": nodes,
                "actions_supported": false,
                "screenshot_supported": false,
            })),
            action_record: None,
        }
    }
}

fn surface_refusal(
    pid: i32,
    surface_token: &str,
    error: crate::ax::surface::ResolveError,
) -> ToolResult {
    let (code, message) = match error {
        crate::ax::surface::ResolveError::Stale => (
            "stale_surface_token",
            "surface token is stale; call list_accessibility_surfaces again",
        ),
        crate::ax::surface::ResolveError::ForeignPid => (
            "surface_pid_mismatch",
            "surface token belongs to another process",
        ),
        crate::ax::surface::ResolveError::InvalidElement => (
            "surface_unavailable",
            "accessibility surface is no longer valid",
        ),
    };
    ToolResult::error(message).with_structured(json!({
        "code": code,
        "effect": "refused",
        "pid": pid,
        "surface": {
            "kind": accessibility_surface::SURFACE_KIND,
            "surface_token": surface_token,
        },
    }))
}

fn bounded_arg(args: &Value, name: &str, default: usize, maximum: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, maximum as u64) as usize)
        .unwrap_or(default)
}

fn project_nodes(
    nodes: Vec<crate::ax::surface::ObservationNode>,
    query: Option<&str>,
) -> Vec<crate::ax::surface::ObservationNode> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return nodes;
    };
    let needle = query.to_ascii_lowercase();
    let mut selected = BTreeSet::new();
    for node in &nodes {
        let haystack = [
            Some(node.role.as_str()),
            node.label.as_deref(),
            node.value.as_deref(),
            node.identifier.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
        if haystack.contains(&needle) {
            let mut current = Some(node.observation_index);
            while let Some(index) = current {
                if !selected.insert(index) {
                    break;
                }
                current = nodes[index].parent_observation_index;
            }
        }
    }
    nodes
        .into_iter()
        .filter(|node| selected.contains(&node.observation_index))
        .collect()
}

fn render_tree(nodes: &[crate::ax::surface::ObservationNode]) -> String {
    let mut output = String::new();
    for node in nodes {
        output.push_str(&"  ".repeat(node.depth));
        output.push_str(&format!(
            "- [observation {}] {}",
            node.observation_index, node.role
        ));
        if let Some(label) = &node.label {
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
        label: &str,
    ) -> crate::ax::surface::ObservationNode {
        crate::ax::surface::ObservationNode {
            observation_index: index,
            parent_observation_index: parent,
            role: role.into(),
            depth: usize::from(parent.is_some()),
            label: Some(label.into()),
            value: None,
            identifier: None,
            frame: None,
            enabled: None,
            selected: None,
        }
    }

    #[test]
    fn query_keeps_matching_nodes_and_ancestors_without_renumbering() {
        let projected = project_nodes(
            vec![
                node(0, None, "AXWindow", "document"),
                node(1, Some(0), "AXStaticText", "status"),
                node(2, Some(0), "AXButton", "save"),
            ],
            Some("save"),
        );
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].observation_index, 0);
        assert_eq!(projected[1].observation_index, 2);
    }

    #[test]
    fn serialized_nodes_have_no_action_handles() {
        let value = serde_json::to_value(node(0, None, "AXStaticText", "hello")).unwrap();
        for forbidden in ["window_id", "element_index", "element_token", "actions"] {
            assert!(value.get(forbidden).is_none());
        }
    }
}
