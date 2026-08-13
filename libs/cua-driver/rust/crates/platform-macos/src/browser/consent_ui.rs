//! Exact macOS handling for Chrome's browser-owned remote-debugging consent.

use std::time::{Duration, Instant};
use std::{collections::HashSet, iter};

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::ax::bindings::{copy_bool_attr, kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::ax::tree::{walk_tree_bounded, AXNode, DEFAULT_MAX_DEPTH};

// Large Chromium pages can put the browser-owned consent sheet after the
// ordinary 2,000-node snapshot cap. Keep this privileged scan bounded while
// allowing enough headroom to inspect Chrome's top-level sheet on pages such
// as Gmail. The matcher below still requires one exact AXSheet and one exact
// structurally unique action before it will press anything.
const CONSENT_MAX_ELEMENTS: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
struct ConsentButtonCandidate {
    element_ptr: usize,
    frame: [f64; 4],
    focused: bool,
}

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn has_nonempty_accessible_text(node: &AXNode) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
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

fn edge_gap(first: [f64; 4], second: [f64; 4]) -> Option<f64> {
    if !first.into_iter().chain(second).all(f64::is_finite)
        || first[2] <= 0.0
        || first[3] <= 0.0
        || second[2] <= 0.0
        || second[3] <= 0.0
        || (first[1] - second[1]).abs() > 1.0
        || (first[3] - second[3]).abs() > 1.0
    {
        return None;
    }
    let first_right = first[0] + first[2];
    let second_right = second[0] + second[2];
    if first_right <= second[0] {
        Some(second[0] - first_right)
    } else if second_right <= first[0] {
        Some(first[0] - second_right)
    } else {
        None
    }
}

fn select_language_independent_allow(
    candidates: &[ConsentButtonCandidate],
) -> Result<usize, BrowserRefusal> {
    if !(2..=3).contains(&candidates.len()) {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent sheet did not expose two or three distinct buttons",
        ));
    }
    let focused = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.focused)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [cancel_index] = focused.as_slice() else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent sheet did not expose exactly one focused cancel action",
        ));
    };
    if candidates.len() == 2 {
        if edge_gap(candidates[0].frame, candidates[1].frame).is_none() {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the native Chromium consent buttons did not form one exact non-overlapping row",
            ));
        }
        return Ok(candidates[1 - cancel_index].element_ptr);
    }

    let mut gaps = Vec::new();
    for first in 0..candidates.len() {
        for second in (first + 1)..candidates.len() {
            if let Some(gap) = edge_gap(candidates[first].frame, candidates[second].frame) {
                gaps.push((gap, first, second));
            }
        }
    }
    gaps.sort_by(|left, right| left.0.total_cmp(&right.0));
    let [(standard_gap, first_standard, second_standard), (extra_gap, _, _), _] = gaps.as_slice()
    else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent buttons did not form one exact non-overlapping row",
        ));
    };
    let max_standard_width =
        candidates[*first_standard].frame[2].max(candidates[*second_standard].frame[2]);
    if *standard_gap >= *extra_gap
        || *standard_gap > max_standard_width
        || *extra_gap < *standard_gap * 2.0
        || ![*first_standard, *second_standard].contains(cancel_index)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent sheet had no uniquely focused standard button pair",
        ));
    }
    let allow_index = if *cancel_index == *first_standard {
        *second_standard
    } else {
        *first_standard
    };
    Ok(candidates[allow_index].element_ptr)
}

fn exact_allow_button_with<F>(
    nodes: &[AXNode],
    mut focused: F,
) -> Result<Option<usize>, BrowserRefusal>
where
    F: FnMut(usize) -> bool,
{
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
        if !sheet_nodes.iter().any(has_nonempty_accessible_text) {
            continue;
        }
        let mut candidates = Vec::new();
        for node in sheet_nodes {
            if node.in_web_content
                || node.role != "AXButton"
                || !node.actions.iter().any(|action| action == "AXPress")
                || node.enabled == Some(false)
                || node.element_index.is_none()
            {
                continue;
            }
            let Some(frame) = node.frame else {
                continue;
            };
            let candidate = ConsentButtonCandidate {
                element_ptr: node.element_ptr,
                frame,
                focused: focused(node.element_ptr),
            };
            if !candidates
                .iter()
                .any(|existing: &ConsentButtonCandidate| existing.frame == frame)
            {
                candidates.push(candidate);
            }
        }
        if (2..=3).contains(&candidates.len()) {
            matches.push(select_language_independent_allow(&candidates)?);
        }
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple native Chromium consent sheets exposed structural allow actions",
        )),
    }
}

fn exact_allow_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    exact_allow_button_with(nodes, |element_ptr| unsafe {
        copy_bool_attr(element_ptr as AXUIElementRef, "AXFocused") == Some(true)
    })
}

fn native_consent_sheet_present(nodes: &[AXNode]) -> bool {
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
        let sheet_nodes = &nodes[sheet_index..end];
        sheet_nodes.iter().any(has_nonempty_accessible_text)
            && (2..=3).contains(
                &sheet_nodes
                    .iter()
                    .filter(|node| {
                        !node.in_web_content
                            && node.role == "AXButton"
                            && node.actions.iter().any(|action| action == "AXPress")
                            && node.enabled != Some(false)
                            && node.element_index.is_some()
                    })
                    .count(),
            )
    })
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
            .any(|nodes| native_consent_sheet_present(nodes));
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
                "multiple Chrome-owned consent sheets exposed structural allow actions",
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

    fn button(pointer: usize, title: &str, x: f64) -> AXNode {
        let mut node = node("AXButton", 2, Some(title), &["AXPress"]);
        node.element_ptr = pointer;
        node.frame = Some([x, 200.0, 90.0, 36.0]);
        node
    }

    fn prompt(title: &str, labels: [&str; 2]) -> Vec<AXNode> {
        vec![
            node("AXWindow", 0, Some("opaque browser window"), &[]),
            node("AXSheet", 1, Some(title), &[]),
            button(7, labels[0], 100.0),
            button(8, labels[1], 202.0),
        ]
    }

    #[test]
    fn matcher_requires_sheet_prompt_and_unique_press_action() {
        let nodes = prompt("Allow remote debugging?", ["Cancel", "Allow"]);
        assert_eq!(
            exact_allow_button_with(&nodes, |pointer| pointer == 7).unwrap(),
            Some(8)
        );
        assert!(native_consent_sheet_present(&nodes));

        let no_sheet = vec![node("AXButton", 1, Some("Allow"), &["AXPress"])];
        assert_eq!(exact_allow_button_with(&no_sheet, |_| false).unwrap(), None);
    }

    #[test]
    fn matcher_treats_sheet_and_button_names_as_opaque_unicode() {
        for (title, labels) in [
            ("e\u{301}", ["हिन्दी", "ไทย"]),
            ("\u{2067}العربية\u{2069}", ["עברית", "فارسی"]),
            ("日本語", ["한국어", "简体中文"]),
            ("Հայերեն", ["ქართული", "አማርኛ"]),
            ("A\u{200d}B", ["👩🏽‍💻", "✅"]),
        ] {
            let nodes = prompt(title, labels);
            assert_eq!(
                exact_allow_button_with(&nodes, |pointer| pointer == 7).unwrap(),
                Some(8)
            );
        }
    }

    #[test]
    fn matcher_refuses_when_accessible_text_cannot_prove_the_native_sheet() {
        let mut nodes = prompt("placeholder", ["A", "B"]);
        for node in &mut nodes {
            node.title = None;
        }

        assert_eq!(
            exact_allow_button_with(&nodes, |pointer| pointer == 7).unwrap(),
            None
        );
    }

    #[test]
    fn matcher_is_direction_independent_for_rtl_layouts() {
        let mut nodes = prompt("\u{2067}العربية\u{2069}", ["إلغاء", "سماح"]);
        nodes[2].frame = Some([202.0, 200.0, 90.0, 36.0]);
        nodes[3].frame = Some([100.0, 200.0, 90.0, 36.0]);
        assert_eq!(
            exact_allow_button_with(&nodes, |pointer| pointer == 7).unwrap(),
            Some(8)
        );
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let nodes = vec![
            node("AXSheet", 1, Some("Allow remote debugging?"), &[]),
            button(7, "A", 100.0),
            button(8, "B", 202.0),
            button(9, "C", 304.0),
        ];
        assert_eq!(
            exact_allow_button_with(&nodes, |pointer| pointer == 7)
                .unwrap_err()
                .code,
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
            vec![7, 9, 8]
        );
    }
}
