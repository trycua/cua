//! Exact macOS setup for Chrome's per-instance remote-debugging toggle.

use std::time::{Duration, Instant};

use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::browser::{BrowserRefusal, BrowserRefusalCode};

use crate::ax::bindings::{
    copy_number_attr, copy_string_attr, focused_element_of_pid, kAXErrorSuccess, perform_action,
    set_bool_attr_true, set_string_attr, AXUIElementRef,
};
use crate::ax::tree::{walk_tree, AXNode};

const SETUP_URL: &str = "chrome://inspect/#remote-debugging";
const SETUP_PAGE_TITLE: &str = "Inspect with Chrome Developer Tools";
const CHECKBOX_LABEL: &str = "Allow remote debugging for this browser instance";

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn field_equals(node: &AXNode, expected: &str) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn has_action(node: &AXNode, action: &str) -> bool {
    node.actions.iter().any(|value| value == action)
}

fn release_actionable_nodes(nodes: &[AXNode]) {
    for node in nodes.iter().filter(|node| node.element_index.is_some()) {
        unsafe { CFRelease(node.element_ptr as CFTypeRef) };
    }
}

fn unique_actionable(
    nodes: &[AXNode],
    role: &str,
    label: &str,
    action: &str,
) -> Result<Option<usize>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| node.role == role && field_equals(node, label) && has_action(node, action))
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("multiple exact {role} controls matched {label:?}"),
        )),
    }
}

fn setup_page_proven(nodes: &[AXNode]) -> bool {
    let exact_url = nodes.iter().any(|node| {
        node.role == "AXTextField"
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(SETUP_URL))
    });
    let exact_page = nodes
        .iter()
        .any(|node| node.role == "AXWebArea" && field_equals(node, SETUP_PAGE_TITLE));
    let exact_heading = nodes
        .iter()
        .any(|node| node.role == "AXHeading" && field_equals(node, "Remote debugging"));
    exact_url && exact_page && exact_heading
}

fn exact_setup_checkbox(nodes: &[AXNode]) -> Result<Option<usize>, BrowserRefusal> {
    if !setup_page_proven(nodes) {
        return Ok(None);
    }
    unique_actionable(nodes, "AXCheckBox", CHECKBOX_LABEL, "AXPress")
}

fn unique_omnibox(nodes: &[AXNode]) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXTextField", "Address and search bar", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved Chrome window has no exact address-and-search field",
        )
    })
}

fn new_tab_button(nodes: &[AXNode]) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXButton", "New Tab", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the approved Chrome window has no exact New Tab button",
        )
    })
}

fn is_same_element(left: usize, right: usize) -> bool {
    unsafe { CFEqual(left as CFTypeRef, right as CFTypeRef) != 0 }
}

fn select_new_tab_close_button(
    before: &[AXNode],
    after: &[AXNode],
    same_element: impl Fn(usize, usize) -> bool,
) -> Result<Option<usize>, BrowserRefusal> {
    let prior_tabs = before
        .iter()
        .filter(|node| node.role == "AXRadioButton" && node.element_index.is_some())
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    let new_tabs = after
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.role == "AXRadioButton"
                && node.element_index.is_some()
                && !prior_tabs
                    .iter()
                    .any(|prior| same_element(*prior, node.element_ptr))
        })
        .collect::<Vec<_>>();
    let (tab_index, tab) = match new_tabs.as_slice() {
        [(index, tab)] => (*index, *tab),
        [] => return Ok(None),
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "Chrome exposed multiple new tab candidates",
            ))
        }
    };
    let end = after
        .iter()
        .enumerate()
        .skip(tab_index + 1)
        .find(|(_, node)| node.depth <= tab.depth)
        .map_or(after.len(), |(index, _)| index);
    let close_buttons = after[tab_index + 1..end]
        .iter()
        .filter(|node| {
            node.role == "AXButton" && field_equals(node, "Close") && has_action(node, "AXPress")
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match close_buttons.as_slice() {
        [element] => Ok(Some(*element)),
        [] => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the newly created Chrome tab has no exact Close button",
        )),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the newly created Chrome tab has multiple Close buttons",
        )),
    }
}

fn new_tab_close_button(
    before: &[AXNode],
    after: &[AXNode],
) -> Result<Option<usize>, BrowserRefusal> {
    let element = select_new_tab_close_button(before, after, is_same_element)?;
    if let Some(element) = element {
        unsafe { CFRetain(element as CFTypeRef) };
    }
    Ok(element)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckboxState {
    Off,
    On,
}

fn checkbox_state(value: Option<f64>) -> Result<CheckboxState, BrowserRefusal> {
    match value {
        Some(value) if value.abs() < f64::EPSILON => Ok(CheckboxState::Off),
        Some(value) if (value - 1.0).abs() < f64::EPSILON => Ok(CheckboxState::On),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the exact remote-debugging checkbox had an unknown checked state",
        )),
    }
}

pub struct SetupUiHandle {
    close_button: Option<usize>,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
}

impl SetupUiHandle {
    fn rollback_remote_debugging(&mut self, pid: i32, window_id: u32) -> bool {
        if !self.enabled_remote_debugging {
            return true;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut pressed_rollback = false;
        loop {
            let tree = walk_tree(pid, Some(window_id), None);
            let checkbox = exact_setup_checkbox(&tree.nodes);
            let result = match checkbox {
                Ok(Some(element)) => {
                    let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                    match checkbox_state(value) {
                        Ok(CheckboxState::Off) => Some(true),
                        Ok(CheckboxState::On) => {
                            if pressed_rollback {
                                None
                            } else {
                                let pressed =
                                    unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                                if pressed == kAXErrorSuccess {
                                    pressed_rollback = true;
                                    None
                                } else {
                                    Some(false)
                                }
                            }
                        }
                        Err(_) => Some(false),
                    }
                }
                Ok(None) => None,
                Err(_) => Some(false),
            };
            release_actionable_nodes(&tree.nodes);
            if let Some(done) = result {
                if done {
                    self.enabled_remote_debugging = false;
                }
                return done;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn abort(mut self, pid: i32, window_id: u32, error: BrowserRefusal) -> BrowserRefusal {
        let enabled_remote_debugging = self.enabled_remote_debugging;
        let restored_remote_debugging = self.rollback_remote_debugging(pid, window_id);
        let opened_setup_page = self.opened_setup_page;
        let focused_setup_address_field = self.focused_setup_address_field;
        let closed_setup_page = self.close().unwrap_or(false);
        let mut error = error;
        let cause = error.detail.take();
        error.with_detail(serde_json::json!({
            "setup_side_effects": {
                "opened_setup_page": opened_setup_page,
                "closed_setup_page": closed_setup_page,
                "focused_setup_address_field": focused_setup_address_field,
                "enabled_remote_debugging": enabled_remote_debugging,
                "restored_remote_debugging": restored_remote_debugging,
            },
            "cause": cause,
        }))
    }

    pub fn close_for_success(
        mut self,
        pid: i32,
        window_id: u32,
    ) -> Result<Option<bool>, BrowserRefusal> {
        let Some(element) = self.close_button.take() else {
            return Ok(None);
        };
        let result = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
        unsafe { CFRelease(element as CFTypeRef) };
        if result == kAXErrorSuccess {
            return Ok(Some(true));
        }
        Err(self.abort(
            pid,
            window_id,
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "Chrome's exact temporary-tab Close action became stale before AXPress",
            ),
        ))
    }

    /// Close the temporary setup tab. `None` means the approved window was
    /// already displaying the exact setup page and no tab was opened.
    pub fn close(mut self) -> Option<bool> {
        let element = self.close_button.take()?;
        let result = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
        unsafe { CFRelease(element as CFTypeRef) };
        Some(result == kAXErrorSuccess)
    }
}

impl Drop for SetupUiHandle {
    fn drop(&mut self) {
        if let Some(element) = self.close_button.take() {
            let _ = unsafe { perform_action(element as AXUIElementRef, "AXPress") };
            unsafe { CFRelease(element as CFTypeRef) };
        }
    }
}

pub fn enable(pid: i32, window_id: u32) -> Result<SetupUiHandle, BrowserRefusal> {
    let initial = walk_tree(pid, Some(window_id), None);
    let initial_checkbox = exact_setup_checkbox(&initial.nodes);
    let mut handle = match initial_checkbox {
        Ok(Some(_)) => SetupUiHandle {
            close_button: None,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
        },
        Ok(None) => {
            let new_tab = new_tab_button(&initial.nodes);
            let new_tab = match new_tab {
                Ok(element) => element,
                Err(error) => {
                    release_actionable_nodes(&initial.nodes);
                    return Err(error);
                }
            };
            let pressed = unsafe { perform_action(new_tab as AXUIElementRef, "AXPress") };
            if pressed != kAXErrorSuccess {
                release_actionable_nodes(&initial.nodes);
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "Chrome's exact New Tab action became stale before AXPress",
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            let (created, close_button) = loop {
                let created = walk_tree(pid, Some(window_id), None);
                match new_tab_close_button(&initial.nodes, &created.nodes) {
                    Ok(Some(element)) => break (created, element),
                    Ok(None) if Instant::now() < deadline => {
                        release_actionable_nodes(&created.nodes);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Ok(None) => {
                        release_actionable_nodes(&initial.nodes);
                        release_actionable_nodes(&created.nodes);
                        return Err(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            "Chrome did not expose exactly one newly created tab",
                        )
                        .with_detail(serde_json::json!({
                            "setup_side_effects": {
                                "opened_setup_page": "unknown",
                                "closed_setup_page": false,
                                "enabled_remote_debugging": false,
                            }
                        })));
                    }
                    Err(error) => {
                        release_actionable_nodes(&initial.nodes);
                        release_actionable_nodes(&created.nodes);
                        return Err(error.with_detail(serde_json::json!({
                            "setup_side_effects": {
                                "opened_setup_page": "unknown",
                                "closed_setup_page": false,
                                "enabled_remote_debugging": false,
                            }
                        })));
                    }
                }
            };
            release_actionable_nodes(&initial.nodes);
            let mut handle = SetupUiHandle {
                close_button: Some(close_button),
                opened_setup_page: true,
                enabled_remote_debugging: false,
                focused_setup_address_field: false,
            };
            let omnibox = unique_omnibox(&created.nodes);
            let omnibox = match omnibox {
                Ok(element) => element,
                Err(error) => {
                    release_actionable_nodes(&created.nodes);
                    return Err(handle.abort(pid, window_id, error));
                }
            };
            let pressed = unsafe { perform_action(omnibox as AXUIElementRef, "AXPress") };
            let focused = unsafe { set_bool_attr_true(omnibox as AXUIElementRef, "AXFocused") };
            let written =
                unsafe { set_string_attr(omnibox as AXUIElementRef, "AXValue", SETUP_URL) };
            if pressed != kAXErrorSuccess
                || focused != kAXErrorSuccess
                || written != kAXErrorSuccess
            {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "Chrome's exact address field rejected the bounded setup navigation",
                    ),
                ));
            }
            handle.focused_setup_address_field = true;
            let exact_focus = unsafe {
                focused_element_of_pid(pid).is_some_and(|focused_element| {
                    let equal = is_same_element(focused_element as usize, omnibox);
                    CFRelease(focused_element as CFTypeRef);
                    equal
                })
            };
            let exact_value = unsafe { copy_string_attr(omnibox as AXUIElementRef, "AXValue") }
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(SETUP_URL));
            if !exact_focus || !exact_value {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(pid, window_id, refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "Chrome's exact address field lost focus before setup navigation could be confirmed",
                )));
            }
            let navigation = crate::input::keyboard::press_key(pid, "return", &[]);
            release_actionable_nodes(&created.nodes);
            if let Err(error) = navigation {
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!("could not confirm Chrome's bounded setup navigation: {error}"),
                    ),
                ));
            }
            handle
        }
        Err(error) => {
            release_actionable_nodes(&initial.nodes);
            return Err(error);
        }
    };
    if !handle.opened_setup_page {
        release_actionable_nodes(&initial.nodes);
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tree = walk_tree(pid, Some(window_id), None);
        let checkbox = exact_setup_checkbox(&tree.nodes);
        match checkbox {
            Ok(Some(element)) => {
                let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                match checkbox_state(value) {
                    Ok(CheckboxState::On) => {
                        release_actionable_nodes(&tree.nodes);
                        return Ok(handle);
                    }
                    Ok(CheckboxState::Off) => {
                        let pressed = if handle.enabled_remote_debugging {
                            kAXErrorSuccess
                        } else {
                            unsafe { perform_action(element as AXUIElementRef, "AXPress") }
                        };
                        release_actionable_nodes(&tree.nodes);
                        if pressed != kAXErrorSuccess {
                            return Err(handle.abort(pid, window_id, refusal(
                                BrowserRefusalCode::BrowserWrongTargetRefused,
                                "the exact remote-debugging checkbox became stale before AXPress",
                            )));
                        }
                        if !handle.enabled_remote_debugging {
                            handle.enabled_remote_debugging = true;
                        }
                    }
                    Err(error) => {
                        release_actionable_nodes(&tree.nodes);
                        return Err(handle.abort(pid, window_id, error));
                    }
                }
            }
            Ok(None) => release_actionable_nodes(&tree.nodes),
            Err(error) => {
                release_actionable_nodes(&tree.nodes);
                return Err(handle.abort(pid, window_id, error));
            }
        }
        if Instant::now() >= deadline {
            return Err(handle.abort(
                pid,
                window_id,
                refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact Chrome remote-debugging setup page did not become ready",
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, title: Option<&str>, value: Option<&str>, actions: &[&str]) -> AXNode {
        AXNode {
            element_index: (!actions.is_empty()).then_some(0),
            role: role.to_owned(),
            title: title.map(str::to_owned),
            value: value.map(str::to_owned),
            description: None,
            identifier: None,
            help: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_ptr: 7,
            depth: 0,
            parent_element_index: None,
            frame: None,
        }
    }

    fn tree_node(role: &str, title: Option<&str>, pointer: usize, depth: usize) -> AXNode {
        let mut node = node(role, title, None, &["AXPress"]);
        node.element_ptr = pointer;
        node.depth = depth;
        node
    }

    #[test]
    fn checkbox_requires_exact_internal_page_proof() {
        let nodes = vec![
            node("AXWebArea", Some(SETUP_PAGE_TITLE), None, &[]),
            node("AXHeading", Some("Remote debugging"), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(SETUP_URL),
                &["AXPress"],
            ),
            node("AXCheckBox", Some(CHECKBOX_LABEL), None, &["AXPress"]),
        ];
        assert_eq!(exact_setup_checkbox(&nodes).unwrap(), Some(7));

        let mut wrong_url = nodes.clone();
        wrong_url[2].value = Some("https://example.test/".to_owned());
        assert_eq!(exact_setup_checkbox(&wrong_url).unwrap(), None);
    }

    #[test]
    fn checkbox_matcher_refuses_ambiguity() {
        let nodes = vec![
            node("AXWebArea", Some(SETUP_PAGE_TITLE), None, &[]),
            node("AXHeading", Some("Remote debugging"), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(SETUP_URL),
                &["AXPress"],
            ),
            node("AXCheckBox", Some(CHECKBOX_LABEL), None, &["AXPress"]),
            node("AXCheckBox", Some(CHECKBOX_LABEL), None, &["AXPress"]),
        ];
        assert_eq!(
            exact_setup_checkbox(&nodes).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn new_tab_cleanup_selects_only_the_new_tabs_close_control() {
        let before = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXButton", Some("Close"), 11, 2),
        ];
        let after = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXButton", Some("Close"), 11, 2),
            tree_node("AXRadioButton", Some("New Tab"), 20, 1),
            tree_node("AXButton", Some("Close"), 21, 2),
        ];
        assert_eq!(
            select_new_tab_close_button(&before, &after, |left, right| left == right).unwrap(),
            Some(21)
        );
    }

    #[test]
    fn new_tab_cleanup_refuses_multiple_new_tabs() {
        let before = vec![tree_node("AXRadioButton", Some("Original"), 10, 1)];
        let after = vec![
            tree_node("AXRadioButton", Some("Original"), 10, 1),
            tree_node("AXRadioButton", Some("New Tab"), 20, 1),
            tree_node("AXButton", Some("Close"), 21, 2),
            tree_node("AXRadioButton", Some("Another"), 30, 1),
            tree_node("AXButton", Some("Close"), 31, 2),
        ];
        assert_eq!(
            select_new_tab_close_button(&before, &after, |left, right| left == right)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn unknown_checkbox_values_refuse() {
        assert_eq!(checkbox_state(Some(0.0)).unwrap(), CheckboxState::Off);
        assert_eq!(checkbox_state(Some(1.0)).unwrap(), CheckboxState::On);
        assert_eq!(
            checkbox_state(None).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(
            checkbox_state(Some(0.5)).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }
}
