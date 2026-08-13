//! Exact Linux AT-SPI handling for Chromium's browser-owned debugging consent.

use std::time::{Duration, Instant};

use cua_driver_core::browser::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserRefusal, BrowserRefusalCode,
};

use crate::atspi::AtspiNode;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConsentButtonCandidate {
    element_index: usize,
    parent_element_index: usize,
    rect: (i32, i32, i32, i32),
    focused: bool,
    depth: usize,
}

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
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

fn trusted_prompt_nodes(nodes: &[AtspiNode]) -> impl DoubleEndedIterator<Item = &AtspiNode> {
    nodes.iter().filter(|node| {
        !node.in_web_content
            && !role_is(node, &["document web", "document frame", "document"])
            && !is_in_web_content(nodes, node)
    })
}

fn has_nonempty_accessible_text(node: &AtspiNode) -> bool {
    [
        node.name.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

fn is_descendant_of(nodes: &[AtspiNode], node: &AtspiNode, ancestor_index: usize) -> bool {
    let mut parent = node.parent_element_index;
    for _ in 0..nodes.len() {
        let Some(parent_index) = parent else {
            return false;
        };
        if parent_index == ancestor_index {
            return true;
        }
        let Some(parent_node) = nodes
            .iter()
            .find(|candidate| candidate.element_index == Some(parent_index))
        else {
            return false;
        };
        parent = parent_node.parent_element_index;
    }
    false
}

fn exact_native_prompt_root(nodes: &[AtspiNode]) -> Result<Option<usize>, BrowserRefusal> {
    let roots = trusted_prompt_nodes(nodes)
        .filter(|node| {
            role_is(node, &["alert"])
                && node.element_index.is_some()
                && has_nonempty_accessible_text(node)
                && node.parent_element_index.is_some_and(|parent_index| {
                    nodes
                        .iter()
                        .find(|candidate| candidate.element_index == Some(parent_index))
                        .is_some_and(|parent| role_is(parent, &["frame", "window"]))
                })
        })
        .filter_map(|root| {
            let root_index = root.element_index?;
            let heading_count = trusted_prompt_nodes(nodes)
                .filter(|node| {
                    is_descendant_of(nodes, node, root_index)
                        && role_is(node, &["heading"])
                        && has_nonempty_accessible_text(node)
                })
                .count();
            let explanatory_surface_present = trusted_prompt_nodes(nodes).any(|node| {
                is_descendant_of(nodes, node, root_index)
                    && !role_is(node, &["heading", "push button", "button"])
                    && has_nonempty_accessible_text(node)
            });
            (heading_count == 1 && explanatory_surface_present).then_some(root_index)
        })
        .collect::<Vec<_>>();
    match roots.as_slice() {
        [] => Ok(None),
        [root] => Ok(Some(*root)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved Chromium window exposed multiple native consent alerts",
        )),
    }
}

fn native_prompt_surface_present(nodes: &[AtspiNode]) -> bool {
    exact_native_prompt_root(nodes).ok().flatten().is_some()
}

fn redacted_native_prompt_snapshot(
    nodes: &[AtspiNode],
    bounds: &[(usize, i32, i32, u32, u32)],
) -> serde_json::Value {
    serde_json::Value::Array(
        trusted_prompt_nodes(nodes)
            .filter(|node| {
                has_nonempty_accessible_text(node)
                    || !node.actions.is_empty()
                    || node.focused == Some(true)
            })
            // Chromium appends browser-owned transient UI after its persistent
            // toolbar controls in the AT-SPI tree. Prefer that end when the
            // redacted diagnostic must stay bounded, otherwise the first 64
            // entries contain only the omnibox/toolbars and hide the prompt
            // topology we need to refuse safely.
            .rev()
            .take(64)
            .map(|node| {
                let parent_role = node.parent_element_index.and_then(|parent_index| {
                    nodes
                        .iter()
                        .find(|candidate| candidate.element_index == Some(parent_index))
                        .map(|parent| parent.role.as_str())
                });
                let node_bounds = node.element_index.and_then(|element_index| {
                    bounds
                        .iter()
                        .find(|(index, ..)| *index == element_index)
                        .map(|(_, x, y, width, height)| {
                            serde_json::json!({
                                "x": x,
                                "y": y,
                                "width": width,
                                "height": height,
                            })
                        })
                });
                serde_json::json!({
                    "role": &node.role,
                    "depth": node.depth,
                    "element_index": node.element_index,
                    "parent_element_index": node.parent_element_index,
                    "parent_role": parent_role,
                    "actions": &node.actions,
                    "focused": node.focused,
                    "enabled": node.enabled,
                    "has_text": has_nonempty_accessible_text(node),
                    "bounds": node_bounds,
                })
            })
            .collect(),
    )
}

fn edge_gap(first: (i32, i32, i32, i32), second: (i32, i32, i32, i32)) -> Option<i64> {
    let (first_left, first_top, first_right, first_bottom) = first;
    let (second_left, second_top, second_right, second_bottom) = second;
    if first_top != second_top || first_bottom != second_bottom {
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

fn select_language_independent_allow(
    candidates: &[ConsentButtonCandidate],
) -> Result<usize, BrowserRefusal> {
    if !(2..=3).contains(&candidates.len()) {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt did not expose two or three distinct dialog buttons",
        ));
    }
    let Some(action_parent) = candidates
        .first()
        .map(|candidate| candidate.parent_element_index)
    else {
        unreachable!("the candidate count was already checked")
    };
    if candidates
        .iter()
        .any(|candidate| candidate.parent_element_index != action_parent)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent buttons did not share one exact action container",
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
            "the native Chromium consent prompt did not expose exactly one focused cancel action",
        ));
    };

    if candidates.len() == 2 {
        if edge_gap(candidates[0].rect, candidates[1].rect).is_none() {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the native Chromium consent buttons did not form one exact non-overlapping row",
            ));
        }
        return Ok(candidates[1 - cancel_index].element_index);
    }

    let mut gaps = Vec::new();
    for first in 0..candidates.len() {
        for second in (first + 1)..candidates.len() {
            if let Some(gap) = edge_gap(candidates[first].rect, candidates[second].rect) {
                gaps.push((gap, first, second));
            }
        }
    }
    gaps.sort_by_key(|(gap, _, _)| *gap);
    let [(standard_gap, first_standard, second_standard), (extra_gap, _, _), _] = gaps.as_slice()
    else {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent buttons did not form one exact non-overlapping row",
        ));
    };
    let first_width = i64::from(candidates[*first_standard].rect.2)
        - i64::from(candidates[*first_standard].rect.0);
    let second_width = i64::from(candidates[*second_standard].rect.2)
        - i64::from(candidates[*second_standard].rect.0);
    if *standard_gap >= *extra_gap
        || *standard_gap > first_width.max(second_width)
        || *extra_gap < standard_gap.saturating_mul(2)
        || ![*first_standard, *second_standard].contains(cancel_index)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native Chromium consent prompt had no uniquely focused standard button pair",
        ));
    }
    let allow_index = if *cancel_index == *first_standard {
        *second_standard
    } else {
        *first_standard
    };
    Ok(candidates[allow_index].element_index)
}

fn exact_allow_button(
    nodes: &[AtspiNode],
    bounds: &[(usize, i32, i32, u32, u32)],
) -> Result<Option<usize>, BrowserRefusal> {
    let Some(prompt_root) = exact_native_prompt_root(nodes)? else {
        return Ok(None);
    };
    let mut candidates = Vec::new();
    for node in trusted_prompt_nodes(nodes).filter(|node| {
        is_descendant_of(nodes, node, prompt_root)
            && role_is(node, &["push button", "button"])
            && !node.actions.is_empty()
            && node.enabled != Some(false)
            && node.element_index.is_some()
            && node.parent_element_index.is_some()
    }) {
        let element_index = node.element_index.expect("filtered actionable index");
        let parent_element_index = node
            .parent_element_index
            .expect("filtered action-container index");
        let Some((_, x, y, width, height)) =
            bounds.iter().find(|(index, ..)| *index == element_index)
        else {
            continue;
        };
        let (Ok(width), Ok(height)) = (i32::try_from(*width), i32::try_from(*height)) else {
            continue;
        };
        if width <= 1 || height <= 1 {
            continue;
        }
        let Some(right) = x.checked_add(width) else {
            continue;
        };
        let Some(bottom) = y.checked_add(height) else {
            continue;
        };
        let candidate = ConsentButtonCandidate {
            element_index,
            parent_element_index,
            rect: (*x, *y, right, bottom),
            focused: node.focused == Some(true),
            depth: node.depth,
        };
        if let Some(existing) = candidates
            .iter_mut()
            .find(|existing: &&mut ConsentButtonCandidate| existing.rect == candidate.rect)
        {
            if candidate.depth > existing.depth {
                *existing = candidate;
            }
        } else {
            candidates.push(candidate);
        }
    }
    select_language_independent_allow(&candidates).map(Some)
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

fn with_target_foreground<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Some(window) =
            crate::wayland::sway_ipc::window_for_id(window_id).filter(|window| window.pid == pid)
        {
            crate::wayland::sway_ipc::with_focused_container(window.id, body)
        } else {
            crate::wayland::shell_helper::with_focused_window(pid, window_id, body)
        }
    } else {
        crate::input::with_x11_foreground(window_id, 80, body)
    }
}

fn exact_button_center(
    bounds: &[(usize, i32, i32, u32, u32)],
    element_index: usize,
) -> anyhow::Result<(i32, i32)> {
    let (_, x, y, width, height) = bounds
        .iter()
        .find(|(index, _, _, width, height)| *index == element_index && *width > 1 && *height > 1)
        .ok_or_else(|| anyhow::anyhow!("the exact Allow action had empty screen bounds"))?;
    let center_x = x
        .checked_add(i32::try_from(width / 2)?)
        .ok_or_else(|| anyhow::anyhow!("Allow button center x overflowed"))?;
    let center_y = y
        .checked_add(i32::try_from(height / 2)?)
        .ok_or_else(|| anyhow::anyhow!("Allow button center y overflowed"))?;
    Ok((center_x, center_y))
}

fn trusted_allow_click(pid: u32, window_id: u64) -> anyhow::Result<()> {
    with_target_foreground(pid, window_id, || {
        let tree = crate::atspi::walk_tree(pid, window_id, None);
        let index = exact_allow_button(&tree.nodes, &tree.bounds)
            .map_err(|error| anyhow::anyhow!(error.message))?
            .ok_or_else(|| {
                anyhow::anyhow!("the exact Chromium remote-debugging consent action became stale")
            })?;
        let (center_x, center_y) = exact_button_center(&tree.bounds, index)?;
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            crate::wayland::click_desktop(center_x, center_y, 1, 1)
        } else {
            crate::input::send_click_xtest_desktop(center_x, center_y, 1, 1)
        }
    })
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
    let mut accessibility_action_at = None;
    let mut trusted_click_attempted = false;
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
        let prompt_present = native_prompt_surface_present(&tree.nodes);
        saw_prompt |= prompt_present;
        match exact_allow_button(&tree.nodes, &tree.bounds)? {
            Some(index) if accessibility_action_at.is_none() => {
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
                accessibility_action_at = Some(Instant::now());
            }
            Some(_)
                if !trusted_click_attempted
                    && accessibility_action_at.is_some_and(|attempted| {
                        attempted.elapsed() >= Duration::from_millis(150)
                    }) =>
            {
                let window_id = request.window_id;
                tokio::task::spawn_blocking(move || trusted_allow_click(pid, window_id))
                    .await
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!(
                                "could not dispatch the trusted browser consent click: {error}"
                            ),
                        )
                    })?
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!("the trusted browser consent click failed: {error}"),
                        )
                    })?;
                trusted_click_attempted = true;
            }
            None if saw_prompt && !prompt_present => {
                if accessibility_action_at.is_some() {
                    return Ok(BrowserConsentOutcome::Accepted);
                }
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
                )
                .with_detail(serde_json::json!({
                    "redacted_native_nodes": redacted_native_prompt_snapshot(
                        &tree.nodes,
                        &tree.bounds,
                    ),
                })));
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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
            enabled: None,
            selected: None,
            focused: None,
            description: None,
            document_uri: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_key: 0,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    fn prompt() -> Vec<AtspiNode> {
        opaque_prompt("Allow remote debugging?", ["Cancel", "Allow"])
    }

    fn prompt_bounds() -> Vec<(usize, i32, i32, u32, u32)> {
        vec![(184, 604, 367, 81, 38), (185, 693, 367, 81, 38)]
    }

    fn opaque_prompt(title: &str, labels: [&str; 2]) -> Vec<AtspiNode> {
        let mut result = chrome_151_prompt();
        result.retain(|node| node.element_index != Some(183));
        result
            .iter_mut()
            .find(|node| node.element_index == Some(176))
            .expect("fixture heading")
            .name = Some(title.to_owned());
        result
            .iter_mut()
            .find(|node| node.element_index == Some(184))
            .expect("fixture cancel")
            .name = Some(labels[0].to_owned());
        result
            .iter_mut()
            .find(|node| node.element_index == Some(185))
            .expect("fixture allow")
            .name = Some(labels[1].to_owned());
        result
    }

    fn indexed_node(
        index: usize,
        role: &str,
        name: &str,
        actions: &[&str],
        parent: Option<usize>,
        depth: usize,
    ) -> AtspiNode {
        let mut result = node(role, name, actions);
        result.element_index = Some(index);
        result.element_key = index as u64;
        result.parent_element_index = parent;
        result.depth = depth;
        result
    }

    fn chrome_151_prompt() -> Vec<AtspiNode> {
        let mut cancel = indexed_node(
            184,
            "push button",
            "opaque cancel",
            &["press", "showContextMenu"],
            Some(182),
            6,
        );
        cancel.focused = Some(true);
        vec![
            indexed_node(0, "frame", "browser", &["doDefault"], None, 0),
            indexed_node(172, "alert", "opaque prompt", &["doDefault"], Some(0), 1),
            indexed_node(173, "panel", "opaque prompt", &["doDefault"], Some(172), 2),
            indexed_node(174, "panel", "opaque prompt", &["doDefault"], Some(173), 3),
            indexed_node(175, "panel", "opaque title", &["doDefault"], Some(174), 4),
            indexed_node(176, "heading", "opaque title", &["doDefault"], Some(175), 5),
            indexed_node(177, "panel", "opaque body", &["doDefault"], Some(174), 4),
            indexed_node(178, "panel", "opaque body", &["doDefault"], Some(177), 5),
            indexed_node(179, "panel", "opaque body", &["doDefault"], Some(178), 6),
            indexed_node(180, "panel", "opaque body", &["doDefault"], Some(179), 7),
            indexed_node(182, "panel", "opaque actions", &["doDefault"], Some(177), 5),
            indexed_node(
                183,
                "push button",
                "opaque settings",
                &["press", "showContextMenu"],
                Some(182),
                6,
            ),
            cancel,
            indexed_node(
                185,
                "push button",
                "opaque allow",
                &["press", "showContextMenu"],
                Some(182),
                6,
            ),
        ]
    }

    fn chrome_151_bounds() -> Vec<(usize, i32, i32, u32, u32)> {
        vec![
            (183, 366, 367, 161, 38),
            (184, 604, 367, 81, 38),
            (185, 693, 367, 81, 38),
        ]
    }

    #[test]
    fn matcher_accepts_real_chrome_151_alert_topology_without_reading_labels() {
        assert_eq!(
            exact_allow_button(&chrome_151_prompt(), &chrome_151_bounds()).unwrap(),
            Some(185)
        );
    }

    #[test]
    fn matcher_ignores_browser_toolbar_buttons_outside_the_exact_alert() {
        let mut nodes = chrome_151_prompt();
        nodes.push(indexed_node(
            50,
            "push button",
            "opaque toolbar action",
            &["click"],
            Some(0),
            1,
        ));
        let mut bounds = chrome_151_bounds();
        bounds.push((50, 20, 20, 32, 32));

        assert_eq!(exact_allow_button(&nodes, &bounds).unwrap(), Some(185));
    }

    #[test]
    fn matcher_refuses_buttons_split_across_action_containers() {
        let mut nodes = chrome_151_prompt();
        nodes
            .iter_mut()
            .find(|node| node.element_index == Some(185))
            .expect("fixture allow")
            .parent_element_index = Some(177);

        assert_eq!(
            exact_allow_button(&nodes, &chrome_151_bounds())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_refuses_multiple_native_alert_roots() {
        let mut nodes = chrome_151_prompt();
        let duplicate_root = indexed_node(
            272,
            "alert",
            "another opaque prompt",
            &["doDefault"],
            Some(0),
            1,
        );
        let duplicate_panel = indexed_node(
            273,
            "panel",
            "another opaque body",
            &["doDefault"],
            Some(272),
            2,
        );
        let duplicate_heading = indexed_node(
            274,
            "heading",
            "another opaque title",
            &["doDefault"],
            Some(273),
            3,
        );
        nodes.extend([duplicate_root, duplicate_panel, duplicate_heading]);

        assert_eq!(
            exact_allow_button(&nodes, &chrome_151_bounds())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_treats_native_prompt_names_as_opaque_unicode() {
        let bounds = prompt_bounds();
        for (title, labels) in [
            ("e\u{301}", ["हिन्दी", "ไทย"]),
            ("\u{2067}العربية\u{2069}", ["עברית", "فارسی"]),
            ("日本語", ["한국어", "简体中文"]),
            ("Հայերեն", ["ქართული", "አማርኛ"]),
            ("A\u{200d}B", ["👩🏽‍💻", "✅"]),
        ] {
            assert_eq!(
                exact_allow_button(&opaque_prompt(title, labels), &bounds).unwrap(),
                Some(185)
            );
        }
    }

    #[test]
    fn matcher_refuses_when_accessible_text_cannot_prove_the_native_surface() {
        let bounds = prompt_bounds();
        let mut nodes = opaque_prompt("placeholder", ["A", "B"]);
        for node in &mut nodes {
            node.name = None;
            node.value = None;
            node.description = None;
        }

        assert_eq!(exact_allow_button(&nodes, &bounds).unwrap(), None);
    }

    #[test]
    fn matcher_requires_exact_security_prompt_and_unique_allow_action() {
        assert_eq!(
            exact_allow_button(&prompt(), &prompt_bounds()).unwrap(),
            Some(185)
        );
        assert!(exact_allow_button(
            &[node("push button", "Allow", &["click"])],
            &prompt_bounds()
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn matcher_refuses_ambiguous_allow_actions() {
        let mut nodes = prompt();
        let mut extra = node("push button", "Anything", &["click"]);
        extra.element_index = Some(186);
        extra.element_key = 186;
        extra.parent_element_index = Some(182);
        extra.depth = 6;
        nodes.push(extra);
        let mut bounds = prompt_bounds();
        bounds.push((186, 782, 367, 81, 38));
        assert_eq!(
            exact_allow_button(&nodes, &bounds).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn matcher_collapses_duplicate_atspi_paths_for_one_physical_button() {
        let mut nodes = prompt();
        let mut duplicate = node("push button", "Allow", &["click"]);
        duplicate.element_index = Some(186);
        duplicate.element_key = 186;
        duplicate.parent_element_index = Some(182);
        duplicate.depth = 7;
        nodes
            .iter_mut()
            .find(|node| node.element_index == Some(185))
            .expect("fixture allow")
            .depth = 6;
        nodes.push(duplicate);
        let bounds = vec![
            (184, 604, 367, 81, 38),
            (185, 693, 367, 81, 38),
            (186, 693, 367, 81, 38),
        ];
        assert_eq!(exact_allow_button(&nodes, &bounds).unwrap(), Some(186));
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
        assert_eq!(exact_allow_button(&nodes, &prompt_bounds()).unwrap(), None);
    }

    #[test]
    fn exact_button_center_requires_nonempty_bounds() {
        assert_eq!(
            exact_button_center(&[(7, 10, 20, 80, 30)], 7).unwrap(),
            (50, 35)
        );
        assert!(exact_button_center(&[(7, 10, 20, 1, 30)], 7).is_err());
        assert!(exact_button_center(&[(8, 10, 20, 80, 30)], 7).is_err());
    }
}
