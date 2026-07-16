//! Exact macOS handling for Chrome's browser-owned remote-debugging consent.

use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::ax::bindings::{kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::ax::tree::{walk_tree, AXNode};

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn normalized_text(node: &AXNode) -> String {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .trim()
    .to_ascii_lowercase()
}

fn release_actionable_nodes(nodes: &[AXNode]) {
    for node in nodes.iter().filter(|node| node.element_index.is_some()) {
        unsafe { CFRelease(node.element_ptr as CFTypeRef) };
    }
}

fn remote_debugging_sheet_present(nodes: &[AXNode]) -> bool {
    nodes.iter().enumerate().any(|(sheet_index, sheet)| {
        if sheet.role != "AXSheet" {
            return false;
        }
        let end = nodes
            .iter()
            .enumerate()
            .skip(sheet_index + 1)
            .find(|(_, node)| node.depth <= sheet.depth)
            .map_or(nodes.len(), |(index, _)| index);
        nodes[sheet_index..end].iter().any(|node| {
            let text = normalized_text(node);
            text.contains("remote debugging") || text.contains("remote-debugging")
        })
    })
}

fn exact_allow_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for (sheet_index, sheet) in nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXSheet")
    {
        let end = nodes
            .iter()
            .enumerate()
            .skip(sheet_index + 1)
            .find(|(_, node)| node.depth <= sheet.depth)
            .map_or(nodes.len(), |(index, _)| index);
        let sheet_nodes = &nodes[sheet_index..end];
        let prompt_is_remote_debugging = sheet_nodes.iter().any(|node| {
            let text = normalized_text(node);
            text.contains("remote debugging") || text.contains("remote-debugging")
        });
        if !prompt_is_remote_debugging {
            continue;
        }
        for node in sheet_nodes {
            if node.role != "AXButton" || !node.actions.iter().any(|action| action == "AXPress") {
                continue;
            }
            let label = normalized_text(node);
            let identifier = node
                .identifier
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let semantic_allow = matches!(label.as_str(), "allow" | "allow remote debugging")
                || (identifier.contains("allow")
                    && (identifier.contains("debug") || identifier.contains("confirm")));
            if semantic_allow {
                matches.push(node.element_ptr);
            }
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple semantic allow actions matched the browser consent sheet",
        )),
    }
}

pub async fn handle(
    request: BrowserConsentRequest,
) -> Result<BrowserConsentOutcome, BrowserRefusal> {
    let pid = i32::try_from(request.pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser pid is outside the macOS process-id range",
        )
    })?;
    let window_id = u32::try_from(request.window_id).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser window is outside the macOS window-id range",
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_prompt = false;
    loop {
        let tree = tokio::task::spawn_blocking(move || walk_tree(pid, Some(window_id), None))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect the browser consent UI: {error}"),
                )
            })?;
        let prompt_present = remote_debugging_sheet_present(&tree.nodes);
        saw_prompt |= prompt_present;
        let candidate = exact_allow_button(&tree.nodes);
        match candidate {
            Ok(Some(element)) => {
                let pressed = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                release_actionable_nodes(&tree.nodes);
                if pressed != kAXErrorSuccess {
                    return Err(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "the exact browser consent action became stale before AXPress",
                    ));
                }
                return Ok(BrowserConsentOutcome::Accepted);
            }
            Ok(None) => release_actionable_nodes(&tree.nodes),
            Err(error) => {
                release_actionable_nodes(&tree.nodes);
                return Err(error);
            }
        }
        if saw_prompt && !prompt_present {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRevoked,
                "the person dismissed the browser consent sheet",
            ));
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "no exact Chrome remote-debugging consent sheet appeared for reconnect attempt {}",
                    request.attempt
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, depth: usize, title: Option<&str>, actions: &[&str]) -> AXNode {
        AXNode {
            element_index: (!actions.is_empty()).then_some(0),
            role: role.to_owned(),
            title: title.map(str::to_owned),
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_ptr: 7,
            depth,
            parent_element_index: None,
            frame: None,
        }
    }

    #[test]
    fn matcher_requires_sheet_prompt_and_unique_press_action() {
        let nodes = vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(7));

        let no_sheet = vec![node("AXButton", 1, Some("Allow"), &["AXPress"])];
        assert_eq!(exact_allow_button(&no_sheet).unwrap(), None);
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let nodes = vec![
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(
            exact_allow_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }
}
