//! Exact macOS handling for Chrome's browser-owned remote-debugging consent.

use std::time::{Duration, Instant};
use std::{collections::HashSet, iter};

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::ax::bindings::{kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::ax::tree::{walk_native_chrome_bounded, AXNode, TreeWalkResult};

// Keep the native-chrome walk bounded even though it skips web content.
// The matcher still requires one exact AXSheet and semantic Allow action.
const CONSENT_MAX_ELEMENTS: usize = 5_000;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn normalized_text(node: &AXNode) -> String {
    let mut parts = Vec::new();
    for text in [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let text = text.trim().to_ascii_lowercase();
        if !text.is_empty() && !parts.contains(&text) {
            parts.push(text);
        }
    }
    parts.join(" ")
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

pub(super) const CHINESE_PROMPT: &str = "要允许远程调试吗？";
const CHINESE_WARNING: &str = "为进行调试，一款外部应用请求完全控制此 Chrome 会话，包括访问您保存的数据、Cookie 和网站数据，以及前往任意网址。";

fn has_text(node: &AXNode, expected: &str) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.trim() == expected)
}

fn sheet_subtree(nodes: &[AXNode], sheet_index: usize) -> &[AXNode] {
    let end = nodes
        .iter()
        .enumerate()
        .skip(sheet_index + 1)
        .find(|(_, node)| node.depth <= nodes[sheet_index].depth)
        .map_or(nodes.len(), |(index, _)| index);
    &nodes[sheet_index..end]
}

fn chinese_sheet(sheet: &[AXNode]) -> bool {
    has_text(&sheet[0], CHINESE_PROMPT)
        && sheet[1..]
            .iter()
            .any(|node| has_text(node, CHINESE_WARNING))
}

fn consent_sheets(nodes: &[AXNode]) -> impl Iterator<Item = &[AXNode]> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXSheet")
        .map(|(index, _)| sheet_subtree(nodes, index))
        .filter(|sheet| !sheet.iter().any(|node| node.in_web_content))
        .filter(|sheet| {
            chinese_sheet(sheet)
                || sheet.iter().any(|node| {
                    let text = normalized_text(node);
                    text.contains("remote debugging") || text.contains("remote-debugging")
                })
        })
}

fn remote_debugging_sheet_present(nodes: &[AXNode]) -> bool {
    consent_sheets(nodes).next().is_some()
}

fn exact_button(nodes: &[AXNode], allow: bool) -> Result<Option<usize>, BrowserRefusal> {
    let mut matches = Vec::new();
    for sheet in consent_sheets(nodes) {
        let chinese = chinese_sheet(sheet);
        let mut allows = Vec::new();
        let mut cancels = Vec::new();
        for node in sheet.iter().filter(|node| {
            node.role == "AXButton" && node.actions.iter().any(|action| action == "AXPress")
        }) {
            let (is_allow, is_cancel) = if chinese {
                (has_text(node, "允许"), has_text(node, "取消"))
            } else {
                let text = normalized_text(node);
                let id = node
                    .identifier
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                (
                    matches!(text.as_str(), "allow" | "allow remote debugging")
                        || (id.contains("allow")
                            && (id.contains("debug") || id.contains("confirm"))),
                    text == "cancel",
                )
            };
            if (is_allow && is_cancel)
                || (chinese && (is_allow || is_cancel) && has_text(node, "在“设置”中关闭"))
            {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "a browser consent button matched conflicting decisions",
                ));
            }
            if is_allow {
                allows.push(node.element_ptr);
            }
            if is_cancel {
                cancels.push(node.element_ptr);
            }
        }
        if chinese && (allows.len() != 1 || cancels.len() != 1) {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "localized browser consent requires unique positive allow and cancel actions",
            ));
        }
        matches.extend(if allow { allows } else { cancels });
    }
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple browser consent actions matched the requested decision",
        )),
    }
}

fn exact_allow_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    exact_button(nodes, true)
}

fn exact_cancel_button(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    exact_button(nodes, false)
}

struct ConsentTrees(Vec<Vec<AXNode>>);

impl ConsentTrees {
    fn push(&mut self, tree: TreeWalkResult) -> Result<(), BrowserRefusal> {
        let incomplete = tree.truncated || tree.walk.truncated();
        self.0.push(tree.nodes);
        if incomplete {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "browser consent observation was truncated",
            ));
        }
        Ok(())
    }
}

impl Drop for ConsentTrees {
    fn drop(&mut self) {
        for nodes in &self.0 {
            release_actionable_nodes(nodes);
        }
    }
}

fn read_consent_trees(pid: i32, window_id: u32) -> Result<ConsentTrees, BrowserRefusal> {
    let mut trees = ConsentTrees(Vec::new());
    for candidate in consent_surface_ids(crate::windows::all_windows(), pid, window_id) {
        trees.push(walk_native_chrome_bounded(
            pid,
            candidate,
            CONSENT_MAX_ELEMENTS,
        ))?;
    }
    Ok(trees)
}

fn settled_after_scan(
    accepted_prompt: bool,
    prompt_present: bool,
    deadline_expired: bool,
    attempt: u8,
) -> Result<Option<BrowserConsentOutcome>, BrowserRefusal> {
    if accepted_prompt && !prompt_present {
        return Ok(Some(BrowserConsentOutcome::Accepted));
    }
    if deadline_expired {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "Chrome remote-debugging consent did not settle for reconnect attempt {attempt}"
            ),
        ));
    }
    Ok(None)
}

/// Dismiss any exact Chrome-owned remote-debugging sheet before teardown.
/// Turning off the setting does not reliably close a sheet that an existing
/// WebSocket connection already presented.
pub fn dismiss(pid: i32, approved_window_id: u32) -> Result<bool, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut dismissed = false;
    loop {
        let trees = read_consent_trees(pid, approved_window_id)?;
        let prompt_present = trees
            .0
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        if !prompt_present {
            return Ok(dismissed);
        }

        let mut candidates = Vec::new();
        for nodes in &trees.0 {
            if let Some(element) = exact_cancel_button(nodes)? {
                candidates.push(element);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let pressed = match candidates.as_slice() {
            [element] => unsafe { perform_action(*element as AXUIElementRef, "AXPress") },
            [] => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact remote-debugging consent sheet exposed no semantic cancel action",
                ));
            }
            _ => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "multiple Chrome-owned remote-debugging consent sheets exposed semantic cancel actions",
                ));
            }
        };
        if pressed != kAXErrorSuccess {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the exact browser consent cancel action became stale before AXPress",
            ));
        }
        drop(trees);
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
        let trees = tokio::task::spawn_blocking(move || read_consent_trees(pid, window_id))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect the browser consent UI: {error}"),
                )
            })??;
        let prompt_present = trees
            .0
            .iter()
            .any(|nodes| remote_debugging_sheet_present(nodes));
        saw_prompt |= prompt_present;
        if let Some(outcome) = settled_after_scan(
            accepted_prompt,
            prompt_present,
            Instant::now() >= deadline,
            request.attempt,
        )? {
            return Ok(outcome);
        }
        let mut candidates = Vec::new();
        for nodes in &trees.0 {
            if let Some(element) = exact_allow_button(nodes)? {
                candidates.push(element);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if let [element] = candidates.as_slice() {
            let pressed = request
                .action
                .perform(|| unsafe { perform_action(*element as AXUIElementRef, "AXPress") });
            drop(trees);
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
        drop(trees);
        if candidates.len() > 1 {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "multiple Chrome-owned remote-debugging consent sheets exposed semantic allow actions",
            ));
        }
        if saw_prompt && !prompt_present {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRevoked,
                "the person dismissed the browser consent sheet",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_closed_sheet_wins_over_elapsed_deadline() {
        assert_eq!(
            settled_after_scan(true, false, true, 1).unwrap(),
            Some(BrowserConsentOutcome::Accepted)
        );
        assert_eq!(
            settled_after_scan(true, true, true, 1).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(
            settled_after_scan(false, false, true, 1).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

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

    #[tokio::test]
    async fn cancelled_scan_releases_late_blocking_result() {
        use core_foundation::base::{CFGetRetainCount, CFRetain, TCFType};
        use core_foundation::string::CFString;

        struct Finished(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Finished {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let value = CFString::new("consent-cancelled-scan-owned-reference");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let scan = tokio::task::spawn_blocking(move || {
            let mut element = node("AXButton", 0, Some("Allow"), &["AXPress"]);
            element.element_ptr = unsafe { CFRetain(ptr as CFTypeRef) } as usize;
            let owned = ConsentTrees(vec![vec![element]]);
            ready_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            // Tuple fields drop in order: release the native reference before signalling.
            (owned, Finished(Some(dropped_tx)))
        });
        ready_rx.await.unwrap();
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        drop(scan);
        finish_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), dropped_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base);
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

    fn captured_chinese_sheet() -> Vec<AXNode> {
        let mut nodes = vec![node("AXSheet", 1, Some("要允许远程调试吗？"), &[])];
        let mut warning = node("AXStaticText", 2, None, &[]);
        warning.value = Some("为进行调试，一款外部应用请求完全控制此 Chrome 会话，包括访问您保存的数据、Cookie 和网站数据，以及前往任意网址。".to_owned());
        nodes.push(warning);
        for (ptr, label) in [(25, "在“设置”中关闭"), (26, "取消"), (27, "允许")] {
            let mut button = node("AXButton", 2, None, &["AXPress"]);
            button.description = Some(label.to_owned());
            button.element_ptr = ptr;
            button.enabled = Some(true);
            nodes.push(button);
        }
        nodes
    }

    #[test]
    fn captured_fields_select_positive_decisions_only_inside_sheet() {
        let mut nodes = captured_chinese_sheet();
        nodes.insert(0, node("AXButton", 1, Some("允许"), &["AXPress"]));
        assert!(remote_debugging_sheet_present(&nodes));
        assert_eq!(exact_allow_button(&nodes).unwrap(), Some(27));
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(26));
        nodes[3].identifier = Some("cancel".to_owned());
        assert_eq!(exact_cancel_button(&nodes).unwrap(), Some(26));
    }

    #[test]
    fn localized_purpose_cannot_come_from_identifier_or_title_alone() {
        for title in ["Save changes?", CHINESE_PROMPT] {
            let mut nodes = captured_chinese_sheet();
            nodes[0].title = Some(title.to_owned());
            nodes[1].value = None;
            nodes[4].identifier = Some("remote-debugging-allow".to_owned());
            assert!(!remote_debugging_sheet_present(&nodes));
            assert_eq!(exact_allow_button(&nodes).unwrap(), None);
            assert_eq!(exact_cancel_button(&nodes).unwrap(), None);
        }
    }

    #[test]
    fn localized_cancel_is_not_the_other_button() {
        let mut nodes = captured_chinese_sheet();
        nodes.remove(3);
        assert_eq!(
            exact_cancel_button(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn localized_decision_ambiguity_conflicts_and_web_content_refuse() {
        for case in 0..4 {
            let mut nodes = captured_chinese_sheet();
            match case {
                0 => nodes.push(nodes[4].clone()),
                1 => nodes.push(nodes[3].clone()),
                2 => nodes[4].title = Some("取消".to_owned()),
                _ => nodes[3].title = Some("在“设置”中关闭".to_owned()),
            }
            for result in [exact_allow_button(&nodes), exact_cancel_button(&nodes)] {
                assert_eq!(
                    result.unwrap_err().code,
                    BrowserRefusalCode::BrowserWrongTargetRefused
                );
            }
        }
        let mut nodes = captured_chinese_sheet();
        nodes[4].in_web_content = true;
        assert!(!remote_debugging_sheet_present(&nodes));
        assert_eq!(exact_allow_button(&nodes).unwrap(), None);
        assert_eq!(exact_cancel_button(&nodes).unwrap(), None);
    }

    #[test]
    fn reported_truncation_releases_all_owned_references_on_early_return() {
        use core_foundation::{
            array::CFArray,
            base::{CFGetRetainCount, CFRetain, TCFType},
            string::CFString,
        };
        use cua_driver_core::walk_budget::WalkBudget;
        let value = CFArray::from_CFTypes(&[CFString::new("owned")]);
        let ptr = value.as_CFTypeRef();
        let baseline = unsafe { CFGetRetainCount(ptr) };
        let result = (|| {
            let mut trees = ConsentTrees(Vec::new());
            for truncated in [false, true] {
                unsafe {
                    CFRetain(ptr);
                }
                let mut button = node("AXButton", 1, Some("Allow"), &["AXPress"]);
                button.element_ptr = ptr as usize;
                let mut budget = WalkBudget::nodes_only(1);
                if truncated {
                    budget.stop_for_timeout();
                }
                trees.push(TreeWalkResult {
                    nodes: vec![button],
                    tree_markdown: String::new(),
                    truncated: false,
                    walk: budget.outcome(),
                    window_scope: None,
                })?;
            }
            Ok::<_, BrowserRefusal>(())
        })();
        assert_eq!(
            result.unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(unsafe { CFGetRetainCount(ptr) }, baseline);
    }
}
