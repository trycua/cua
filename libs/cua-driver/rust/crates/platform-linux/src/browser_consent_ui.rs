//! Exact Linux AT-SPI handling for Chromium's browser-owned debugging consent.

use std::time::{Duration, Instant};

use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::atspi::AtspiNode;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn normalized_text(node: &AtspiNode) -> String {
    [
        node.name.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .trim()
    .to_ascii_lowercase()
}

fn role_is(node: &AtspiNode, accepted: &[&str]) -> bool {
    let role = node.role.trim().to_ascii_lowercase();
    accepted.iter().any(|candidate| role == *candidate)
}

fn is_in_web_content(nodes: &[AtspiNode], node: &AtspiNode) -> bool {
    let mut parent = node.parent_element_index;
    for _ in 0..nodes.len() {
        let Some(parent_index) = parent else {
            return false;
        };
        let Some(parent_node) = nodes
            .iter()
            .find(|candidate| candidate.element_index == Some(parent_index))
        else {
            return false;
        };
        if role_is(parent_node, &["document web", "document frame", "document"]) {
            return true;
        }
        parent = parent_node.parent_element_index;
    }
    true
}

fn trusted_prompt_nodes(nodes: &[AtspiNode]) -> impl Iterator<Item = &AtspiNode> {
    nodes.iter().filter(|node| {
        !node.in_web_content
            && !role_is(node, &["document web", "document frame", "document"])
            && !is_in_web_content(nodes, node)
    })
}

fn remote_debugging_prompt_present(nodes: &[AtspiNode]) -> bool {
    let has_title =
        trusted_prompt_nodes(nodes).any(|node| normalized_text(node) == "allow remote debugging?");
    let body = trusted_prompt_nodes(nodes)
        .map(normalized_text)
        .collect::<Vec<_>>()
        .join(" ");
    has_title
        && body.contains("external app wants full control")
        && body.contains("saved data, cookies and site data")
        && body.contains("navigate to any url")
}

fn exact_allow_button(
    nodes: &[AtspiNode],
    bounds: &[(usize, i32, i32, u32, u32)],
) -> Result<Option<usize>, BrowserRefusal> {
    if !remote_debugging_prompt_present(nodes) {
        return Ok(None);
    }
    let matches = trusted_prompt_nodes(nodes)
        .filter(|node| {
            role_is(node, &["push button", "button"])
                && normalized_text(node) == "allow"
                && !node.actions.is_empty()
                && node.element_index.is_some()
        })
        .map(|node| {
            (
                node.element_index.expect("filtered actionable index"),
                node.element_key,
                node.depth,
                node.role.clone(),
                node.actions.clone(),
                node.parent_element_index,
            )
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        let candidate_bounds = |element_index: usize| {
            bounds
                .iter()
                .find(|(index, _, _, width, height)| {
                    *index == element_index && *width > 0 && *height > 0
                })
                .map(|(_, x, y, width, height)| (*x, *y, *width, *height))
        };
        let first_bounds = candidate_bounds(matches[0].0);
        let same_physical_control = first_bounds.is_some()
            && matches.iter().all(|candidate| {
                candidate.3 == matches[0].3
                    && candidate.4 == matches[0].4
                    && candidate_bounds(candidate.0) == first_bounds
            });
        if same_physical_control {
            return Ok(matches
                .iter()
                .max_by_key(|candidate| candidate.2)
                .map(|candidate| candidate.0));
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [(index, ..)] => Ok(Some(*index)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple exact Allow actions matched the browser consent prompt",
        )
        .with_detail(serde_json::json!({
            "candidates": matches.iter().map(|(element_index, element_key, depth, role, actions, parent_element_index)| {
                let parent = parent_element_index.and_then(|parent_index| nodes.iter().find(|node| node.element_index == Some(parent_index)));
                let bounds = bounds.iter().find(|(index, ..)| index == element_index);
                serde_json::json!({
                "element_index": element_index,
                "element_key": element_key,
                "depth": depth,
                "role": role,
                "actions": actions,
                "parent_element_index": parent_element_index,
                "parent_role": parent.map(|node| node.role.as_str()),
                "parent_name": parent.and_then(|node| node.name.as_deref()),
                "bounds": bounds.map(|(_, x, y, width, height)| serde_json::json!({"x": x, "y": y, "width": width, "height": height})),
            })}).collect::<Vec<_>>()
        }))),
    }
}

fn prove_window_owner(pid: u32, window_id: u64) -> Result<(), BrowserRefusal> {
    let owned = crate::wayland::list_windows_dispatch(Some(pid))
        .into_iter()
        .any(|window| window.xid == window_id);
    if !owned {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            "the approved browser window changed ownership before consent",
        ));
    }
    Ok(())
}

pub async fn handle(
    request: BrowserConsentRequest,
) -> Result<BrowserConsentOutcome, BrowserRefusal> {
    let pid = u32::try_from(request.pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser pid is outside the Linux process-id range",
        )
    })?;
    prove_window_owner(pid, request.window_id)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_prompt = false;
    loop {
        prove_window_owner(pid, request.window_id)?;
        let window_id = request.window_id;
        let tree =
            tokio::task::spawn_blocking(move || crate::atspi::walk_tree(pid, window_id, None))
                .await
                .map_err(|error| {
                    refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("could not inspect the browser consent UI: {error}"),
                    )
                })?;
        let prompt_present = remote_debugging_prompt_present(&tree.nodes);
        saw_prompt |= prompt_present;
        match exact_allow_button(&tree.nodes, &tree.bounds)? {
            Some(index) => {
                tokio::task::spawn_blocking(move || crate::atspi::perform_action(pid, index))
                    .await
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!("could not dispatch the exact browser consent action: {error}"),
                        )
                    })?
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!("the exact browser consent action failed: {error}"),
                        )
                    })?;
                return Ok(BrowserConsentOutcome::Accepted);
            }
            None if saw_prompt && !prompt_present => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserConsentRevoked,
                    "the person dismissed the browser consent prompt",
                ));
            }
            None if Instant::now() >= deadline => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!(
                        "no exact Chromium remote-debugging consent prompt appeared for reconnect attempt {}",
                        request.attempt
                    ),
                ));
            }
            None => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, name: &str, actions: &[&str]) -> AtspiNode {
        AtspiNode {
            element_index: (!actions.is_empty()).then_some(7),
            role: role.to_owned(),
            name: Some(name.to_owned()),
            value: None,
            checked: None,
            description: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_key: 0,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    fn prompt() -> Vec<AtspiNode> {
        vec![
            node("heading", "Allow remote debugging?", &[]),
            node(
                "static",
                "An external app wants full control. This includes access to your saved data, cookies and site data, and the ability to navigate to any URL.",
                &[],
            ),
            node("push button", "Cancel", &["click"]),
            node("push button", "Allow", &["click"]),
        ]
    }

    #[test]
    fn matcher_requires_exact_security_prompt_and_unique_allow_action() {
        assert_eq!(exact_allow_button(&prompt(), &[]).unwrap(), Some(7));
        assert!(
            exact_allow_button(&[node("push button", "Allow", &["click"])], &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let mut nodes = prompt();
        nodes.push(node("push button", "Allow", &["click"]));
        assert_eq!(
            exact_allow_button(&nodes, &[]).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_collapses_duplicate_atspi_paths_for_one_physical_button() {
        let mut nodes = prompt();
        let mut duplicate = node("push button", "Allow", &["click"]);
        duplicate.element_index = Some(8);
        duplicate.element_key = 8;
        duplicate.depth = 2;
        nodes.last_mut().unwrap().depth = 1;
        nodes.push(duplicate);
        let bounds = vec![(7, 10, 20, 80, 30), (8, 10, 20, 80, 30)];
        assert_eq!(exact_allow_button(&nodes, &bounds).unwrap(), Some(8));
    }

    #[test]
    fn matcher_ignores_a_spoofed_prompt_inside_web_content() {
        let mut nodes = prompt();
        let mut document = node("document web", "Example page", &[]);
        document.element_index = Some(42);
        nodes.insert(0, document);
        for child in &mut nodes[1..] {
            child.parent_element_index = Some(42);
            child.in_web_content = true;
        }
        assert_eq!(exact_allow_button(&nodes, &[]).unwrap(), None);
    }
}
