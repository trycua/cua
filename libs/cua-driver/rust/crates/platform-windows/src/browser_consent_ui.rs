//! Exact Windows UIA handling for Chromium's browser-owned debugging consent.

use std::time::{Duration, Instant};

use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationInvokePattern, UIA_InvokePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use crate::uia::UiaNode;

const CHROMIUM_DIALOG_BUTTON_CLASS: &str = "MdTextButton";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConsentButtonCandidate {
    element_ptr: usize,
    rect: (i32, i32, i32, i32),
    has_keyboard_focus: bool,
}

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn release_nodes(nodes: &[UiaNode]) {
    for node in nodes.iter().filter(|node| node.element_ptr != 0) {
        unsafe { drop(IUIAutomationElement::from_raw(node.element_ptr as *mut _)) };
    }
}

fn is_in_web_content(nodes: &[UiaNode], node: &UiaNode) -> bool {
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
        if parent_node.control_type.eq_ignore_ascii_case("Document") {
            return true;
        }
        parent = parent_node.parent_element_index;
    }
    true
}

fn is_trusted_prompt_node(nodes: &[UiaNode], node: &UiaNode) -> bool {
    !node.in_web_content
        && !node.control_type.eq_ignore_ascii_case("Document")
        && !is_in_web_content(nodes, node)
}

fn node_name(node: &UiaNode) -> Option<&str> {
    node.name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn subtree_end(nodes: &[UiaNode], root_index: usize) -> usize {
    let root_depth = nodes[root_index].depth;
    nodes
        .iter()
        .enumerate()
        .skip(root_index + 1)
        .find(|(_, node)| node.depth <= root_depth)
        .map_or(nodes.len(), |(index, _)| index)
}

fn has_matching_pane_ancestor(nodes: &[UiaNode], window_index: usize, name: &str) -> bool {
    let mut descendant_depth = nodes[window_index].depth;
    for ancestor in nodes[..window_index].iter().rev() {
        if ancestor.depth >= descendant_depth {
            continue;
        }
        descendant_depth = ancestor.depth;
        if is_trusted_prompt_node(nodes, ancestor)
            && ancestor.control_type == "Pane"
            && node_name(ancestor) == Some(name)
        {
            return true;
        }
    }
    false
}

fn has_pane_rooted_title_binding(nodes: &[UiaNode], pane_index: usize, name: &str) -> bool {
    let pane = &nodes[pane_index];
    let end = subtree_end(nodes, pane_index);
    let descendants = &nodes[(pane_index + 1)..end];
    let has_direct_title = descendants.iter().any(|node| {
        is_trusted_prompt_node(nodes, node)
            && node.depth == pane.depth + 1
            && node.control_type == "Text"
            && node_name(node) == Some(name)
    });
    let has_nested_title = descendants.iter().any(|node| {
        is_trusted_prompt_node(nodes, node)
            && node.depth > pane.depth + 1
            && node.control_type == "Text"
            && node_name(node) == Some(name)
    });
    let has_nested_window = descendants
        .iter()
        .any(|node| is_trusted_prompt_node(nodes, node) && node.control_type == "Window");
    has_direct_title && has_nested_title && !has_nested_window
}

fn native_prompt_surfaces(nodes: &[UiaNode]) -> Vec<(usize, usize)> {
    nodes
        .iter()
        .enumerate()
        .filter_map(|(root_index, root)| {
            if !is_trusted_prompt_node(nodes, root) || root.depth == 0 {
                return None;
            }
            let name = node_name(root)?;
            let end = subtree_end(nodes, root_index);
            match root.control_type.as_str() {
                "Window" => {
                    if !has_matching_pane_ancestor(nodes, root_index, name) {
                        return None;
                    }
                    nodes[(root_index + 1)..end]
                        .iter()
                        .any(|node| {
                            is_trusted_prompt_node(nodes, node)
                                && node.control_type == "Text"
                                && node_name(node) == Some(name)
                        })
                        .then_some((root_index, end))
                }
                "Pane" if root.depth > 1 => has_pane_rooted_title_binding(nodes, root_index, name)
                    .then_some((root_index, end)),
                _ => None,
            }
        })
        .collect()
}

fn native_prompt_surface_present(nodes: &[UiaNode]) -> bool {
    !native_prompt_surfaces(nodes).is_empty()
}

fn native_button_properties(element_ptr: usize) -> Result<(String, bool), BrowserRefusal> {
    let element = unsafe { IUIAutomationElement::from_raw(element_ptr as *mut _) };
    let properties = unsafe {
        element.CurrentClassName().and_then(|class_name| {
            element
                .CurrentHasKeyboardFocus()
                .map(|focused| (class_name.to_string(), focused.as_bool()))
        })
    };
    std::mem::forget(element);
    properties.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("could not prove a native Chromium consent button: {error}"),
        )
    })
}

fn edge_gap(first: (i32, i32, i32, i32), second: (i32, i32, i32, i32)) -> Option<i64> {
    let (first_left, first_top, first_right, first_bottom) = first;
    let (second_left, second_top, second_right, second_bottom) = second;
    let same_row = first_top == second_top && first_bottom == second_bottom;
    if !same_row {
        return None;
    }
    if first_right <= second_left {
        Some(i64::from(second_left) - i64::from(first_right))
    } else if second_right <= first_left {
        Some(i64::from(first_left) - i64::from(second_right))
    } else {
        None
    }
}

fn select_language_independent_actions(
    candidates: &[ConsentButtonCandidate],
) -> Result<(usize, usize), BrowserRefusal> {
    // Chromium builds this modal from one separated extra action plus the
    // standard OK/Cancel pair. Accessible names are localized, and the whole
    // footer mirrors for RTL locales, but adjacency and separation are stable.
    // Cancel focus is additional contradiction evidence when the browser is
    // active; an inactive browser legitimately reports no focused button.
    if candidates.len() != 3 {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt did not expose exactly three distinct dialog buttons",
        ));
    }
    let focused_indices = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.has_keyboard_focus)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if focused_indices.len() > 1 {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt exposed multiple focused dialog buttons",
        ));
    }

    let mut pairwise_gaps = Vec::new();
    for first in 0..candidates.len() {
        for second in (first + 1)..candidates.len() {
            if let Some(gap) = edge_gap(candidates[first].rect, candidates[second].rect) {
                pairwise_gaps.push((gap, first, second));
            }
        }
    }
    pairwise_gaps.sort_by_key(|(gap, _, _)| *gap);
    let [(standard_gap, first_standard, second_standard), (extra_gap, _, _), _] =
        pairwise_gaps.as_slice()
    else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent buttons did not form one exact non-overlapping dialog row",
        ));
    };
    let first_width = i64::from(candidates[*first_standard].rect.2)
        - i64::from(candidates[*first_standard].rect.0);
    let second_width = i64::from(candidates[*second_standard].rect.2)
        - i64::from(candidates[*second_standard].rect.0);
    if *standard_gap >= *extra_gap
        || *standard_gap > first_width.max(second_width)
        || *extra_gap < standard_gap.saturating_mul(2)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt had no uniquely separated standard button pair",
        ));
    }

    let extra_index = (0..candidates.len())
        .find(|index| index != first_standard && index != second_standard)
        .expect("three candidates and a two-button pair");
    let first_to_extra = edge_gap(
        candidates[*first_standard].rect,
        candidates[extra_index].rect,
    );
    let second_to_extra = edge_gap(
        candidates[*second_standard].rect,
        candidates[extra_index].rect,
    );
    let allow_index = match (first_to_extra, second_to_extra) {
        (Some(first_gap), Some(second_gap)) if first_gap < second_gap => *first_standard,
        (Some(first_gap), Some(second_gap)) if second_gap < first_gap => *second_standard,
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the native Chromium consent prompt had no unique standard button adjacent to the extra action",
            ));
        }
    };
    let cancel_index = if allow_index == *first_standard {
        *second_standard
    } else {
        *first_standard
    };
    if focused_indices
        .first()
        .is_some_and(|focused| *focused != cancel_index)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt focus contradicted the structural cancel button",
        ));
    }
    Ok((
        candidates[allow_index].element_ptr,
        candidates[cancel_index].element_ptr,
    ))
}

fn exact_consent_actions_with<F>(
    nodes: &[UiaNode],
    mut properties: F,
) -> Result<Option<(usize, usize)>, BrowserRefusal>
where
    F: FnMut(usize) -> Result<(String, bool), BrowserRefusal>,
{
    let surfaces = native_prompt_surfaces(nodes);
    if surfaces.is_empty() {
        return Ok(None);
    }
    let mut matches = Vec::new();
    for (start, end) in surfaces {
        let mut candidates = Vec::new();
        for node in nodes[(start + 1)..end].iter().filter(|node| {
            is_trusted_prompt_node(nodes, node)
                && node.control_type == "Button"
                && node.actions.iter().any(|action| action == "invoke")
                && node.element_ptr != 0
                && node.rect.is_some()
        }) {
            let (class_name, has_keyboard_focus) = properties(node.element_ptr)?;
            if class_name != CHROMIUM_DIALOG_BUTTON_CLASS {
                continue;
            }
            let rect = node.rect.expect("filtered above");
            if rect.0 >= rect.2 || rect.1 >= rect.3 {
                continue;
            }
            if candidates
                .iter()
                .any(|candidate: &ConsentButtonCandidate| candidate.rect == rect)
            {
                continue;
            }
            candidates.push(ConsentButtonCandidate {
                element_ptr: node.element_ptr,
                rect,
                has_keyboard_focus,
            });
        }
        matches.push(select_language_independent_actions(&candidates)?);
    }
    match matches.as_slice() {
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple bound native Chromium consent prompts exposed structural allow actions",
        )),
    }
}

fn exact_allow_button_with<F>(
    nodes: &[UiaNode],
    properties: F,
) -> Result<Option<usize>, BrowserRefusal>
where
    F: FnMut(usize) -> Result<(String, bool), BrowserRefusal>,
{
    Ok(exact_consent_actions_with(nodes, properties)?.map(|actions| actions.0))
}

fn exact_allow_button(nodes: &[UiaNode]) -> Result<Option<usize>, BrowserRefusal> {
    Ok(exact_consent_actions_with(nodes, native_button_properties)?.map(|actions| actions.0))
}

fn exact_cancel_button(nodes: &[UiaNode]) -> Result<Option<usize>, BrowserRefusal> {
    Ok(exact_consent_actions_with(nodes, native_button_properties)?.map(|actions| actions.1))
}

unsafe fn invoke(element_ptr: usize) -> Result<(), BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_InvokePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationInvokePattern>())
        .and_then(|pattern| pattern.Invoke());
    std::mem::forget(element);
    result.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("the exact browser consent action failed: {error}"),
        )
    })
}

fn prove_window_owner(hwnd: u64, pid: u32) -> Result<(), BrowserRefusal> {
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut owner)) };
    if owner != pid {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            "the approved browser window changed ownership before consent",
        ));
    }
    Ok(())
}

pub fn dismiss(pid: u32, hwnd: u64) -> Result<bool, BrowserRefusal> {
    prove_window_owner(hwnd, pid)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut dismissed = false;
    loop {
        prove_window_owner(hwnd, pid)?;
        let tree = crate::uia::walk_tree(hwnd, None);
        let prompt_present = native_prompt_surface_present(&tree.nodes);
        if !prompt_present {
            release_nodes(&tree.nodes);
            return Ok(dismissed);
        }
        let cancel = exact_cancel_button(&tree.nodes);
        let invoked = match cancel {
            Ok(Some(element)) => unsafe { invoke(element) },
            Ok(None) => Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the exact remote-debugging consent prompt exposed no structural cancel action",
            )),
            Err(error) => Err(error),
        };
        release_nodes(&tree.nodes);
        invoked?;
        dismissed = true;
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the remote-debugging consent prompt remained after its exact cancel action",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub async fn handle(
    request: BrowserConsentRequest,
) -> Result<BrowserConsentOutcome, BrowserRefusal> {
    let pid = u32::try_from(request.pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved browser pid is outside the Windows process-id range",
        )
    })?;
    prove_window_owner(request.window_id, pid)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_prompt = false;
    loop {
        prove_window_owner(request.window_id, pid)?;
        let hwnd = request.window_id;
        let tree = tokio::task::spawn_blocking(move || crate::uia::walk_tree(hwnd, None))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect the browser consent UI: {error}"),
                )
            })?;
        let prompt_present = native_prompt_surface_present(&tree.nodes);
        saw_prompt |= prompt_present;
        match exact_allow_button(&tree.nodes) {
            Ok(Some(element)) => {
                let invoked = unsafe { invoke(element) };
                release_nodes(&tree.nodes);
                invoked?;
                return Ok(BrowserConsentOutcome::Accepted);
            }
            Ok(None) => release_nodes(&tree.nodes),
            Err(error) => {
                release_nodes(&tree.nodes);
                return Err(error);
            }
        }
        if saw_prompt && !prompt_present {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRevoked,
                "the person dismissed the browser consent prompt",
            ));
        }
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "no exact Chromium remote-debugging consent prompt appeared for reconnect attempt {}",
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

    fn node(control_type: &str, name: &str, actions: &[&str]) -> UiaNode {
        UiaNode {
            element_index: None,
            control_type: control_type.to_owned(),
            name: Some(name.to_owned()),
            value: None,
            automation_id: None,
            help_text: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            enabled: None,
            selected: None,
            element_ptr: 0,
            center_x: 0,
            center_y: 0,
            rect: None,
            msaa_role: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    fn dialog_node(control_type: &str, name: &str, depth: usize) -> UiaNode {
        let mut node = node(control_type, name, &[]);
        node.depth = depth;
        node
    }

    fn button(name: &str, element_ptr: usize, rect: (i32, i32, i32, i32)) -> UiaNode {
        let mut node = node("Button", name, &["invoke"]);
        node.element_index = Some(element_ptr);
        node.element_ptr = element_ptr;
        node.rect = Some(rect);
        node.depth = 8;
        node
    }

    fn prompt(title: &str, labels: [&str; 3]) -> Vec<UiaNode> {
        vec![
            dialog_node("Pane", title, 2),
            dialog_node("Window", title, 3),
            dialog_node("Text", title, 7),
            button(labels[0], 11, (-6066, 343, -5886, 399)),
            button(labels[1], 12, (-5657, 343, -5560, 399)),
            button(labels[2], 13, (-5549, 343, -5452, 399)),
        ]
    }

    fn pane_rooted_prompt(title: &str, labels: [&str; 3]) -> Vec<UiaNode> {
        vec![
            dialog_node("Pane", title, 2),
            dialog_node("Text", title, 3),
            dialog_node("Text", title, 7),
            dialog_node("Text", "opaque explanatory surface", 10),
            button(labels[0], 11, (282, 293, 419, 325)),
            button(labels[1], 12, (574, 293, 629, 325)),
            button(labels[2], 13, (637, 293, 698, 325)),
        ]
    }

    fn properties(element_ptr: usize) -> Result<(String, bool), BrowserRefusal> {
        Ok((CHROMIUM_DIALOG_BUTTON_CLASS.to_owned(), element_ptr == 13))
    }

    #[test]
    fn matcher_is_language_independent_across_localized_native_dialogs() {
        for (title, labels) in [
            (
                "リモート デバッグを許可しますか？",
                ["設定でオフ", "許可", "キャンセル"],
            ),
            (
                "Remote-Debugging zulassen?",
                ["In Einstellungen deaktivieren", "Zulassen", "Abbrechen"],
            ),
            (
                "Autoriser le débogage à distance ?",
                ["Désactiver", "Autoriser", "Annuler"],
            ),
            (
                "هل تريد السماح بتصحيح الأخطاء عن بُعد؟",
                ["تعطيل", "سماح", "إلغاء"],
            ),
        ] {
            assert_eq!(
                exact_allow_button_with(&prompt(title, labels), properties).unwrap(),
                Some(12)
            );
        }
    }

    #[test]
    fn cancel_matcher_selects_the_structural_default_action() {
        let nodes = prompt(
            "Allow remote debugging?",
            ["Turn off in settings", "Allow", "Cancel"],
        );
        assert_eq!(
            exact_consent_actions_with(&nodes, properties).unwrap(),
            Some((12, 13))
        );
    }

    #[test]
    fn matcher_supports_pane_rooted_native_edge_prompt() {
        assert_eq!(
            exact_allow_button_with(
                &pane_rooted_prompt(
                    "¿Permitir la depuración remota?",
                    ["Desactivar", "Permitir", "Cancelar"],
                ),
                properties,
            )
            .unwrap(),
            Some(12)
        );
    }

    #[test]
    fn pane_rooted_prompt_requires_both_title_bindings_and_no_nested_window() {
        let mut missing_direct = pane_rooted_prompt("opaque title", ["A", "B", "C"]);
        missing_direct[1].name = Some("different direct title".to_owned());
        assert_eq!(
            exact_allow_button_with(&missing_direct, properties).unwrap(),
            None
        );

        let mut missing_nested = pane_rooted_prompt("opaque title", ["A", "B", "C"]);
        missing_nested[2].name = Some("different nested title".to_owned());
        assert_eq!(
            exact_allow_button_with(&missing_nested, properties).unwrap(),
            None
        );

        let mut nested_window = pane_rooted_prompt("opaque title", ["A", "B", "C"]);
        nested_window.insert(2, dialog_node("Window", "unrelated native window", 3));
        assert_eq!(
            exact_allow_button_with(&nested_window, properties).unwrap(),
            None
        );

        let mut browser_root = pane_rooted_prompt("opaque title", ["A", "B", "C"]);
        browser_root[0].depth = 1;
        assert_eq!(
            exact_allow_button_with(&browser_root, properties).unwrap(),
            None
        );
    }

    #[test]
    fn matcher_refuses_multiple_pane_rooted_native_prompts() {
        let mut nodes = pane_rooted_prompt("first opaque title", ["A", "B", "C"]);
        nodes.extend(pane_rooted_prompt("second opaque title", ["D", "E", "F"]));

        assert_eq!(
            exact_allow_button_with(&nodes, properties)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_treats_nonempty_unicode_names_as_opaque_data() {
        for (title, labels) in [
            ("e\u{301}", ["é", "E\u{301}", "ë"]),
            ("हिन्दी", ["ไทย", "עברית", "فارسی"]),
            ("日本語", ["한국어", "简体中文", "繁體中文"]),
            ("\u{2067}العربية\u{2069}", ["\u{2066}A\u{2069}", "👩🏽‍💻", "𐐷"]),
            ("Հայերեն", ["ქართული", "አማርኛ", "বাংলা"]),
            ("A\u{200d}B", ["無", "⠿", "✅"]),
        ] {
            assert_eq!(
                exact_allow_button_with(&prompt(title, labels), properties).unwrap(),
                Some(12)
            );
        }
    }

    #[test]
    fn matcher_refuses_when_accessible_names_cannot_bind_the_native_surface() {
        let mut nodes = prompt("placeholder", ["A", "B", "C"]);
        for node in &mut nodes {
            node.name = None;
        }

        assert_eq!(exact_allow_button_with(&nodes, properties).unwrap(), None);
    }

    #[test]
    fn matcher_deduplicates_repeated_uia_walk_rows_by_exact_geometry() {
        let mut nodes = prompt("任何语言", ["A", "B", "C"]);
        let mut duplicate = nodes[4].clone();
        duplicate.element_ptr = 99;
        nodes.push(duplicate);
        assert_eq!(
            exact_allow_button_with(&nodes, properties).unwrap(),
            Some(12)
        );
    }

    #[test]
    fn matcher_is_direction_independent_for_rtl_dialog_layouts() {
        let mut nodes = prompt(
            "هل تريد السماح بتصحيح الأخطاء عن بُعد؟",
            ["تعطيل", "سماح", "إلغاء"],
        );
        nodes[3].rect = Some((600, 343, 780, 399));
        nodes[4].rect = Some((299, 343, 396, 399));
        nodes[5].rect = Some((191, 343, 288, 399));
        assert_eq!(
            exact_allow_button_with(&nodes, properties).unwrap(),
            Some(12)
        );
    }

    #[test]
    fn matcher_does_not_require_focus_when_the_browser_is_inactive() {
        let nodes = prompt(
            "¿Permitir la depuración remota?",
            ["Desactivar", "Permitir", "Cancelar"],
        );
        assert_eq!(
            exact_allow_button_with(&nodes, |_| {
                Ok((CHROMIUM_DIALOG_BUTTON_CLASS.to_owned(), false))
            })
            .unwrap(),
            Some(12)
        );
    }

    #[test]
    fn matcher_refuses_ambiguous_focus_or_button_geometry() {
        let nodes = prompt("Qualsiasi lingua", ["A", "B", "C"]);
        assert_eq!(
            exact_allow_button_with(&nodes, |element_ptr| {
                Ok((CHROMIUM_DIALOG_BUTTON_CLASS.to_owned(), element_ptr >= 12))
            })
            .unwrap_err()
            .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );

        let mut equidistant = nodes;
        equidistant[3].rect = Some((-5700, 343, -5560, 399));
        assert_eq!(
            exact_allow_button_with(&equidistant, properties)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_ignores_a_spoofed_prompt_inside_web_content() {
        let mut nodes = prompt("Permitir depuração remota?", ["A", "B", "C"]);
        for child in &mut nodes {
            child.in_web_content = true;
        }
        assert_eq!(exact_allow_button_with(&nodes, properties).unwrap(), None);
    }

    #[test]
    fn matcher_refuses_chromium_buttons_outside_the_bound_prompt_subtree() {
        let title = "opaque consent surface";
        let nodes = vec![
            dialog_node("Pane", title, 2),
            dialog_node("Window", title, 3),
            dialog_node("Text", title, 7),
            dialog_node("Pane", "unrelated native sibling", 2),
            button("A", 11, (-6066, 343, -5886, 399)),
            button("B", 12, (-5657, 343, -5560, 399)),
            button("C", 13, (-5549, 343, -5452, 399)),
        ];

        assert_eq!(
            exact_allow_button_with(&nodes, properties)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_requires_the_matching_pane_to_be_a_window_ancestor() {
        let title = "opaque consent surface";
        let nodes = vec![
            dialog_node("Pane", title, 2),
            dialog_node("Pane", "different native container", 2),
            dialog_node("Window", title, 3),
            dialog_node("Text", title, 7),
            button("A", 11, (-6066, 343, -5886, 399)),
            button("B", 12, (-5657, 343, -5560, 399)),
            button("C", 13, (-5549, 343, -5452, 399)),
        ];

        assert_eq!(exact_allow_button_with(&nodes, properties).unwrap(), None);
    }

    #[test]
    fn matcher_requires_the_native_prompt_surface_and_chromium_button_class() {
        let mut nodes = prompt("원격 디버깅을 허용하시겠습니까?", ["A", "B", "C"]);
        nodes[0].name = Some("different native pane".to_owned());
        assert_eq!(exact_allow_button_with(&nodes, properties).unwrap(), None);

        let nodes = prompt("원격 디버깅을 허용하시겠습니까?", ["A", "B", "C"]);
        assert_eq!(
            exact_allow_button_with(&nodes, |element_ptr| {
                Ok((
                    if element_ptr == 12 {
                        "RendererButton"
                    } else {
                        CHROMIUM_DIALOG_BUTTON_CLASS
                    }
                    .to_owned(),
                    element_ptr == 13,
                ))
            })
            .unwrap_err()
            .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }
}
