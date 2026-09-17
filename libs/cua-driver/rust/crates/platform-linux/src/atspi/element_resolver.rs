use super::{AtspiIdentity, AtspiNode};
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
pub fn identity_for_node(node: &AtspiNode) -> Vec<u8> {
    serde_json::to_vec(&(
        &node.role,
        &node.name,
        &node.description,
        &node.actions,
        node.depth,
        node.in_web_content,
    ))
    .expect("AT-SPI identity tuple")
}
pub async fn resolve_element_args(
    pid: i32,
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_id: Option<&str>,
    window_id: Option<u64>,
    tool_name: &str,
) -> Result<ResolvedElement<AtspiIdentity>, ToolResult> {
    match element_token::resolve_native(
        pid,
        element_index,
        element_token,
        snapshot_id,
        window_id,
        tool_name,
        move |window, target| resolve_fresh(pid, window, target),
    )
    .await?
    {
        ResolvedElement::Element {
            window_id,
            via_token,
            element: (element_index, element),
            ..
        } => Ok(ResolvedElement::Element {
            window_id,
            element_index,
            via_token,
            element,
        }),
        ResolvedElement::None => Ok(ResolvedElement::None),
    }
}
pub(crate) fn resolve_fresh(
    pid: i32,
    window_id: u64,
    target: &ElementTarget,
) -> Result<Option<(usize, AtspiIdentity)>, String> {
    let tree = super::walk_tree(pid as u32, window_id, None);
    if !tree.trusted {
        return Err("current accessibility state is unavailable from AT-SPI".into());
    }
    if !tree.window_scoped {
        return Err(format!(
            "current accessibility state is not scoped to window_id {window_id}"
        ));
    }
    target
        .resolve_unique(
            tree.nodes.iter().filter_map(|node| {
                node.element_index
                    .map(|index| (identity_for_node(node), (index, node.identity.clone())))
            }),
            tree.complete,
        )?
        .map(|(index, identity)| {
            identity
                .map(|identity| (index, identity))
                .ok_or_else(|| "matched element has no native AT-SPI identity".into())
        })
        .transpose()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn node(i: usize, name: &str) -> AtspiNode {
        AtspiNode {
            element_index: Some(i),
            role: "button".into(),
            name: Some(name.into()),
            value: None,
            checked: None,
            enabled: Some(true),
            selected: None,
            description: None,
            actions: vec!["click".into()],
            element_key: i as u64,
            identity: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }
    #[test]
    fn semantic_identity_ignores_traversal_index() {
        assert_eq!(
            identity_for_node(&node(1, "Save")),
            identity_for_node(&node(9, "Save"))
        );
        assert_ne!(
            identity_for_node(&node(1, "Save")),
            identity_for_node(&node(1, "Cancel"))
        );
    }
}
