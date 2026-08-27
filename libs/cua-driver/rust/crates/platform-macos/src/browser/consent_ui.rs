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

#[derive(Clone, Copy, Debug, PartialEq)]
struct ConsentButtonCandidate {
    element_ptr: usize,
    frame: [f64; 4],
}

fn subtree_end(nodes: &[AXNode], root_index: usize) -> usize {
    let root_depth = nodes[root_index].depth;
    nodes
        .iter()
        .enumerate()
        .skip(root_index + 1)
        .find(|(_, node)| node.depth <= root_depth)
        .map_or(nodes.len(), |(index, _)| index)
}

fn edge_gap(first: [f64; 4], second: [f64; 4]) -> Option<f64> {
    let [first_x, first_y, first_width, first_height] = first;
    let [second_x, second_y, second_width, second_height] = second;
    let same_row = (first_y - second_y).abs() <= 1.0 && (first_height - second_height).abs() <= 1.0;
    if !same_row {
        return None;
    }
    let first_right = first_x + first_width;
    let second_right = second_x + second_width;
    if first_right <= second_x {
        Some(second_x - first_right)
    } else if second_right <= first_x {
        Some(first_x - second_right)
    } else {
        None
    }
}

fn frame_is_inside(inner: [f64; 4], outer: [f64; 4]) -> bool {
    let [inner_x, inner_y, inner_width, inner_height] = inner;
    let [outer_x, outer_y, outer_width, outer_height] = outer;
    inner_width > 0.0
        && inner_height > 0.0
        && inner_x >= outer_x - 1.0
        && inner_y >= outer_y - 1.0
        && inner_x + inner_width <= outer_x + outer_width + 1.0
        && inner_y + inner_height <= outer_y + outer_height + 1.0
}

fn structural_consent_actions(
    sheet: &AXNode,
    sheet_nodes: &[AXNode],
) -> Result<Option<(usize, usize)>, BrowserRefusal> {
    let sheet_title = normalized_text(sheet);
    if sheet_title.is_empty()
        || sheet_nodes
            .iter()
            .filter(|node| node.role == "AXHeading" && normalized_text(node) == sheet_title)
            .count()
            != 1
        || sheet_nodes
            .iter()
            .filter(|node| node.role == "AXStaticText")
            .count()
            < 2
    {
        return Ok(None);
    }
    let Some(sheet_frame) = sheet.frame else {
        return Ok(None);
    };
    let mut candidates = sheet_nodes
        .iter()
        .filter_map(|node| {
            let frame = node.frame?;
            (node.role == "AXButton"
                && node.actions.iter().any(|action| action == "AXPress")
                && node.element_ptr != 0
                && frame_is_inside(frame, sheet_frame))
            .then_some(ConsentButtonCandidate {
                element_ptr: node.element_ptr,
                frame,
            })
        })
        .collect::<Vec<_>>();
    candidates.dedup_by(|first, second| first.frame == second.frame);
    if candidates.len() != 3 {
        return Ok(None);
    }

    let mut pairwise_gaps = Vec::new();
    for first in 0..candidates.len() {
        for second in (first + 1)..candidates.len() {
            if let Some(gap) = edge_gap(candidates[first].frame, candidates[second].frame) {
                pairwise_gaps.push((gap, first, second));
            }
        }
    }
    pairwise_gaps.sort_by(|first, second| first.0.total_cmp(&second.0));
    let [(standard_gap, first_standard, second_standard), (extra_gap, _, _), _] =
        pairwise_gaps.as_slice()
    else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent buttons did not form one exact non-overlapping row",
        ));
    };
    let first_width = candidates[*first_standard].frame[2];
    let second_width = candidates[*second_standard].frame[2];
    if standard_gap >= extra_gap
        || *standard_gap > first_width.max(second_width)
        || *extra_gap < standard_gap * 2.0
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent sheet had no uniquely separated standard button pair",
        ));
    }

    let extra_index = (0..candidates.len())
        .find(|index| index != first_standard && index != second_standard)
        .expect("three candidates and a two-button pair");
    let first_to_extra = edge_gap(
        candidates[*first_standard].frame,
        candidates[extra_index].frame,
    );
    let second_to_extra = edge_gap(
        candidates[*second_standard].frame,
        candidates[extra_index].frame,
    );
    // AppKit places the default action at the outer edge of the standard pair.
    // The whole footer mirrors in RTL locales, so distance from the separated
    // settings action is stable while left/right ordering is not.
    let allow_index = match (first_to_extra, second_to_extra) {
        (Some(first_gap), Some(second_gap)) if first_gap > second_gap => *first_standard,
        (Some(first_gap), Some(second_gap)) if second_gap > first_gap => *second_standard,
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the native Chromium consent sheet had no unique outer default action",
            ));
        }
    };
    let cancel_index = if allow_index == *first_standard {
        *second_standard
    } else {
        *first_standard
    };
    Ok(Some((
        candidates[allow_index].element_ptr,
        candidates[cancel_index].element_ptr,
    )))
}

fn remote_debugging_sheet_present(nodes: &[AXNode]) -> bool {
    nodes.iter().enumerate().any(|(sheet_index, sheet)| {
        if sheet.role != "AXSheet" {
            return false;
        }
        let end = subtree_end(nodes, sheet_index);
        let sheet_nodes = &nodes[sheet_index..end];
        sheet_nodes.iter().any(|node| {
            let text = normalized_text(node);
            text.contains("remote debugging") || text.contains("remote-debugging")
        }) || structural_consent_actions(sheet, sheet_nodes)
            .ok()
            .flatten()
            .is_some()
    })
}

fn exact_allow_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for (sheet_index, sheet) in nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXSheet")
    {
        let end = subtree_end(nodes, sheet_index);
        let sheet_nodes = &nodes[sheet_index..end];
        let prompt_is_remote_debugging = sheet_nodes.iter().any(|node| {
            let text = normalized_text(node);
            text.contains("remote debugging") || text.contains("remote-debugging")
        });
        if !prompt_is_remote_debugging {
            if let Some((allow, _)) = structural_consent_actions(sheet, sheet_nodes)? {
                matches.push(allow);
            }
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

fn exact_cancel_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for (sheet_index, sheet) in nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXSheet")
    {
        let end = subtree_end(nodes, sheet_index);
        let sheet_nodes = &nodes[sheet_index..end];
        let prompt_is_remote_debugging = sheet_nodes.iter().any(|node| {
            let text = normalized_text(node);
            text.contains("remote debugging") || text.contains("remote-debugging")
        });
        if !prompt_is_remote_debugging {
            if let Some((_, cancel)) = structural_consent_actions(sheet, sheet_nodes)? {
                matches.push(cancel);
            }
            continue;
        }
        for node in sheet_nodes {
            if node.role == "AXButton"
                && node.actions.iter().any(|action| action == "AXPress")
                && normalized_text(node) == "cancel"
            {
                matches.push(node.element_ptr);
            }
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple semantic cancel actions matched the browser consent sheet",
        )),
    }
}

/// Dismiss any exact Chrome-owned remote-debugging sheet before teardown.
/// Turning off the setting does not reliably close a sheet that an existing
/// WebSocket connection already presented.
pub fn dismiss(pid: i32, approved_window_id: u32) -> Result<bool, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut dismissed = false;
    loop {
        let trees = consent_surface_ids(crate::windows::all_windows(), pid, approved_window_id)
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
            .collect::<Vec<_>>();
        let prompt_present = trees
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        if !prompt_present {
            for nodes in &trees {
                release_actionable_nodes(nodes);
            }
            return Ok(dismissed);
        }

        let mut candidates = Vec::new();
        let mut matcher_error = None;
        for nodes in &trees {
            match exact_cancel_button(nodes) {
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
        let pressed = match candidates.as_slice() {
            [element] => unsafe { perform_action(*element as AXUIElementRef, "AXPress") },
            [] => {
                for nodes in &trees {
                    release_actionable_nodes(nodes);
                }
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact remote-debugging consent sheet exposed no semantic cancel action",
                ));
            }
            _ => {
                for nodes in &trees {
                    release_actionable_nodes(nodes);
                }
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "multiple Chrome-owned remote-debugging consent sheets exposed semantic cancel actions",
                ));
            }
        };
        for nodes in &trees {
            release_actionable_nodes(nodes);
        }
        if pressed != kAXErrorSuccess {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the exact browser consent cancel action became stale before AXPress",
            ));
        }
        dismissed = true;
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the remote-debugging consent sheet remained after its exact cancel action",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
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
            in_web_content: false,
        }
    }

    fn framed_button(title: &str, element_ptr: usize, frame: [f64; 4]) -> AXNode {
        let mut button = node("AXButton", 2, Some(title), &["AXPress"]);
        button.element_ptr = element_ptr;
        button.frame = Some(frame);
        button
    }

    fn localized_prompt(title: &str, labels: [&str; 3]) -> Vec<AXNode> {
        let mut sheet = node("AXSheet", 1, Some(title), &[]);
        sheet.frame = Some([0.0, 0.0, 448.0, 200.0]);
        vec![
            sheet,
            node("AXHeading", 2, Some(title), &[]),
            node("AXStaticText", 2, Some("localized warning"), &[]),
            node("AXStaticText", 2, Some("localized safety note"), &[]),
            framed_button(labels[0], 11, [20.0, 160.0, 123.0, 36.0]),
            framed_button(labels[1], 12, [292.0, 160.0, 64.0, 36.0]),
            framed_button(labels[2], 13, [364.0, 160.0, 64.0, 36.0]),
        ]
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
    fn cancel_matcher_requires_remote_debugging_sheet() {
        let nodes = vec![
            node("AXWindow", 0, Some("Chrome"), &[]),
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
            node("AXButton", 2, Some("Allow"), &["AXPress"]),
        ];
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(7));

        let unrelated = vec![
            node("AXSheet", 1, Some("Save changes?"), &[]),
            node("AXButton", 2, Some("Cancel"), &["AXPress"]),
        ];
        assert_eq!(exact_cancel_button(&unrelated).unwrap(), None);
    }

    #[test]
    fn matcher_is_language_independent_for_localized_chromium_sheet() {
        let nodes = localized_prompt("要允许远程调试吗？", ["在“设置”中关闭", "取消", "允许"]);
        assert!(remote_debugging_sheet_present(&nodes));
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(13));
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(12));
    }

    #[test]
    fn structural_matcher_supports_mirrored_rtl_button_order() {
        let mut nodes = localized_prompt(
            "هل تريد السماح بتصحيح الأخطاء عن بُعد؟",
            ["تعطيل", "إلغاء", "سماح"],
        );
        nodes[4].frame = Some([305.0, 160.0, 123.0, 36.0]);
        nodes[5].frame = Some([92.0, 160.0, 64.0, 36.0]);
        nodes[6].frame = Some([20.0, 160.0, 64.0, 36.0]);
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(13));
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(12));
    }

    #[test]
    fn structural_matcher_requires_the_sheet_heading_binding() {
        let mut nodes = localized_prompt("要允许远程调试吗？", ["在“设置”中关闭", "取消", "允许"]);
        nodes[1].title = Some("不同的确认对话框".to_owned());
        assert!(!remote_debugging_sheet_present(&nodes));
        assert_eq!(exact_allow_button(&nodes).unwrap(), None);
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
            current_space_id: None,
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
