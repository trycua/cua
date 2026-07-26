//! Exact macOS handling for Chrome's browser-owned remote-debugging consent.

use std::time::{Duration, Instant};
use std::{collections::HashSet, iter};

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::ax::bindings::{kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::ax::tree::{walk_tree_bounded, AXNode, DEFAULT_MAX_DEPTH};

// Large Chromium pages can put the browser-owned consent sheet after the
// ordinary 2,000-node snapshot cap. Keep this privileged scan bounded while
// allowing enough headroom to inspect Chrome's top-level sheet on pages such
// as Gmail. The matcher below still requires one exact AXSheet and one exact
// semantic Allow action before it will press anything.
const CONSENT_MAX_ELEMENTS: usize = 5_000;

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

fn consent_surface_ids(
    windows: impl IntoIterator<Item = crate::windows::WindowInfo>,
    pid: i32,
    approved_window_id: u32,
) -> Vec<u32> {
    let mut windows = windows
        .into_iter()
        .filter(|window| {
            window.pid == pid
                && window.title != "Allow remote debugging?"
                && !window.title.trim().is_empty()
                && window.bounds.width > 0.0
                && window.bounds.height > 0.0
        })
        .collect::<Vec<_>>();
    windows.sort_by_key(|window| std::cmp::Reverse(window.z_index));
    let mut seen = HashSet::new();
    iter::once(approved_window_id)
        .chain(windows.into_iter().map(|window| window.window_id))
        .filter(|window_id| seen.insert(*window_id))
        .collect()
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
    let mut accepted_prompt = false;
    loop {
        let trees = tokio::task::spawn_blocking(move || {
            consent_surface_ids(crate::windows::all_windows(), pid, window_id)
                .into_iter()
                .map(|candidate_window_id| {
                    walk_tree_bounded(
                        pid,
                        Some(candidate_window_id),
                        None,
                        CONSENT_MAX_ELEMENTS,
                        DEFAULT_MAX_DEPTH,
                    )
                    .nodes
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect the browser consent UI: {error}"),
            )
        })?;
        let prompt_present = trees
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        saw_prompt |= prompt_present;
        let mut candidates = Vec::new();
        let mut matcher_error = None;
        for nodes in &trees {
            match exact_allow_button(nodes) {
                Ok(Some(element)) => candidates.push(element),
                Ok(None) => {}
                Err(error) => {
                    matcher_error = Some(error);
                    break;
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if let Some(error) = matcher_error {
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            return Err(error);
        }
        if let [element] = candidates.as_slice() {
            let pressed = unsafe { perform_action(*element as AXUIElementRef, "AXPress") };
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            if pressed != kAXErrorSuccess {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact browser consent action became stale before AXPress",
                ));
            }
            // Chrome can queue more than one browser-owned consent sheet when
            // an earlier connection attempt was interrupted. Do not report
            // acceptance merely because AXPress returned success: keep
            // inspecting the exact approved process until every matching
            // sheet is gone, or the bounded deadline/ambiguity checks refuse.
            accepted_prompt = true;
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        for nodes in &trees {
            release_actionable_nodes(nodes);
        }
        if candidates.len() > 1 {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "multiple Chrome-owned remote-debugging consent sheets exposed semantic allow actions",
            ));
        }
        if accepted_prompt && !prompt_present {
            return Ok(BrowserConsentOutcome::Accepted);
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
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: None,
            selected: None,
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

    #[test]
    fn consent_surfaces_keep_approved_window_then_frontmost_normal_windows() {
        let window = |window_id, title: &str, z_index| crate::windows::WindowInfo {
            window_id,
            pid: 42,
            app_name: "Google Chrome".to_owned(),
            title: title.to_owned(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 1200.0,
                height: 800.0,
            },
            layer: 0,
            z_index,
            is_on_screen: true,
            on_current_space: Some(true),
            space_ids: None,
        };
        assert_eq!(
            consent_surface_ids(
                [
                    window(7, "Approved", 10),
                    window(8, "Frontmost", 30),
                    window(9, "Allow remote debugging?", 40),
                ],
                42,
                7,
            ),
            vec![7, 8]
        );
    }
}
