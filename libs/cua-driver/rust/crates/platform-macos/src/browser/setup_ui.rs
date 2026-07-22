//! Exact macOS setup for Chrome's per-instance remote-debugging toggle.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::browser::{
    BrowserProduct, BrowserRefusal, BrowserRefusalCode, BrowserSetupDescriptor,
    EXISTING_PROFILE_SETUP_READY_TIMEOUT,
};

use crate::ax::bindings::{
    copy_number_attr, copy_string_attr, element_screen_center, focused_element_of_pid,
    kAXErrorSuccess, perform_action, set_bool_attr_true, set_string_attr, AXUIElementRef,
};
use crate::ax::tree::{walk_tree, AXNode};

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

fn setup_page_proven(nodes: &[AXNode], descriptor: &BrowserSetupDescriptor) -> bool {
    let exact_url = nodes.iter().any(|node| {
        node.role == "AXTextField"
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
    });
    let exact_page = nodes.iter().any(|node| {
        node.role == "AXWebArea"
            && descriptor
                .page_titles
                .iter()
                .any(|title| field_equals(node, title))
    });
    let exact_heading = nodes
        .iter()
        .any(|node| node.role == "AXHeading" && field_equals(node, descriptor.page_heading));
    exact_url && exact_page && exact_heading
}

fn exact_setup_checkbox(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    if !setup_page_proven(nodes, descriptor) {
        return Ok(None);
    }
    unique_actionable(nodes, "AXCheckBox", descriptor.checkbox_label, "AXPress")
}

fn unique_omnibox(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXTextField", "Address and search bar", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the approved {} window has no exact address-and-search field",
                descriptor.product_name
            ),
        )
    })
}

fn is_exact_setup_suggestion(value: &str, setup_url: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case(setup_url)
        || value.strip_prefix(setup_url).is_some_and(|suffix| {
            suffix.eq_ignore_ascii_case(", press Tab then Enter to Remove Suggestion.")
        })
}

fn node_is_exact_setup_suggestion(node: &AXNode, setup_url: &str) -> bool {
    [
        node.title.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| is_exact_setup_suggestion(value, setup_url))
}

fn exact_omnibox_suggestion(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    let omniboxes = nodes
        .iter()
        .filter(|node| {
            node.role == "AXTextField"
                && field_equals(node, "Address and search bar")
                && node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
        })
        .count();
    match omniboxes {
        0 => return Ok(None),
        1 => {}
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{} exposed multiple exact address fields containing the setup URL",
                    descriptor.product_name
                ),
            ))
        }
    }
    let popups = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.role == "AXWebArea" && field_equals(node, "Omnibox Popup"))
        .collect::<Vec<_>>();
    let (popup_index, popup) = match popups.as_slice() {
        [] => return Ok(None),
        [(index, popup)] => (*index, *popup),
        _ => {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{} exposed multiple exact omnibox suggestion popups",
                    descriptor.product_name
                ),
            ))
        }
    };
    let end = nodes
        .iter()
        .enumerate()
        .skip(popup_index + 1)
        .find(|(_, node)| node.depth <= popup.depth)
        .map_or(nodes.len(), |(index, _)| index);
    let matches = nodes[popup_index + 1..end]
        .iter()
        .filter(|node| {
            node.role == "AXMenuItem"
                && node_is_exact_setup_suggestion(node, descriptor.setup_url)
                && has_action(node, "AXPress")
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{} exposed multiple exact setup URL suggestions",
                descriptor.product_name
            ),
        )),
    }
}

fn new_tab_button(
    nodes: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<usize, BrowserRefusal> {
    unique_actionable(nodes, "AXButton", "New Tab", "AXPress")?.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the approved {} window has no exact New Tab button",
                descriptor.product_name
            ),
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
    descriptor: &BrowserSetupDescriptor,
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
                format!(
                    "{} exposed multiple new tab candidates",
                    descriptor.product_name
                ),
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
            node.role == "AXButton"
                && descriptor
                    .tab_close_labels
                    .iter()
                    .any(|label| field_equals(node, label))
                && has_action(node, "AXPress")
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match close_buttons.as_slice() {
        [element] => Ok(Some(*element)),
        [] => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the newly created {} tab has no exact Close button",
                descriptor.product_name
            ),
        )),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "the newly created {} tab has multiple Close buttons",
                descriptor.product_name
            ),
        )),
    }
}

fn new_tab_close_button(
    before: &[AXNode],
    after: &[AXNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    let element = select_new_tab_close_button(before, after, is_same_element, descriptor)?;
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
    descriptor: &'static BrowserSetupDescriptor,
    close_button: Option<usize>,
    enable_attempted: bool,
    trusted_checkbox_fallback_attempted: bool,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
}

impl SetupUiHandle {
    fn rollback_remote_debugging(&mut self, pid: i32, window_id: u32) -> bool {
        if !self.enabled_remote_debugging {
            return true;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut pressed_rollback = false;
        let mut trusted_fallback_attempted = false;
        loop {
            let tree = walk_tree(pid, Some(window_id), None);
            let checkbox = exact_setup_checkbox(&tree.nodes, self.descriptor);
            let result = match checkbox {
                Ok(Some(element)) => {
                    let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                    match checkbox_state(value) {
                        Ok(CheckboxState::Off) => Some(true),
                        Ok(CheckboxState::On) => {
                            if !pressed_rollback {
                                let pressed =
                                    unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                                if pressed == kAXErrorSuccess {
                                    pressed_rollback = true;
                                    None
                                } else {
                                    Some(false)
                                }
                            } else if self.descriptor.product == BrowserProduct::MicrosoftEdge
                                && !trusted_fallback_attempted
                            {
                                let center =
                                    unsafe { element_screen_center(element as AXUIElementRef) };
                                trusted_fallback_attempted = true;
                                release_actionable_nodes(&tree.nodes);
                                let Some((x, y)) = center else {
                                    return false;
                                };
                                self.foregrounded_window = true;
                                self.injected_global_input = true;
                                let restored = crate::input::skylight::with_foreground_assist(
                                    pid,
                                    window_id,
                                    || {
                                        std::thread::sleep(Duration::from_millis(60));
                                        if crate::apps::frontmost_pid() != Some(pid) {
                                            anyhow::bail!(
                                                "the approved browser lost foreground before the rollback click"
                                            );
                                        }
                                        crate::input::mouse::click_at_xy_desktop_preserving_cursor(
                                            x, y,
                                        )?;
                                        std::thread::sleep(Duration::from_millis(60));
                                        Ok(())
                                    },
                                )
                                .unwrap_or(false);
                                if !restored {
                                    return false;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                                continue;
                            } else {
                                None
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
        let foregrounded_window = self.foregrounded_window;
        let injected_global_input = self.injected_global_input;
        let closed_setup_page = self.close().unwrap_or(false);
        let mut error = error;
        let cause = error.detail.take();
        error.with_detail(serde_json::json!({
            "setup_side_effects": {
                "opened_setup_page": opened_setup_page,
                "closed_setup_page": closed_setup_page,
                "focused_setup_address_field": focused_setup_address_field,
                "enabled_remote_debugging": enabled_remote_debugging,
                "foregrounded_window": foregrounded_window,
                "injected_global_input": injected_global_input,
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
        let product_name = self.descriptor.product_name;
        Err(self.abort(
            pid,
            window_id,
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "{}'s exact temporary-tab Close action became stale before AXPress",
                    product_name
                ),
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

type PendingSetupKey = (i32, u32);

fn pending_setups() -> &'static Mutex<HashMap<PendingSetupKey, SetupUiHandle>> {
    static PENDING: OnceLock<Mutex<HashMap<PendingSetupKey, SetupUiHandle>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn retain_pending(
    pid: i32,
    window_id: u32,
    handle: SetupUiHandle,
) -> Result<(), BrowserRefusal> {
    let mut pending = pending_setups().lock().unwrap();
    if pending.contains_key(&(pid, window_id)) {
        drop(pending);
        return Err(handle.abort(
            pid,
            window_id,
            refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "another approved browser setup is already pending for this exact window",
            ),
        ));
    }
    pending.insert((pid, window_id), handle);
    Ok(())
}

pub fn commit_pending(pid: i32, window_id: u32) -> Result<bool, BrowserRefusal> {
    let handle = pending_setups()
        .lock()
        .unwrap()
        .remove(&(pid, window_id))
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "the exact pending browser setup cleanup handle is missing",
            )
        })?;
    Ok(handle.close_for_success(pid, window_id)?.unwrap_or(false))
}

pub fn abort_pending(pid: i32, window_id: u32, error: BrowserRefusal) -> BrowserRefusal {
    match pending_setups().lock().unwrap().remove(&(pid, window_id)) {
        Some(handle) => handle.abort(pid, window_id, error),
        None => error.with_detail(serde_json::json!({
            "setup_cleanup": "the exact pending browser setup cleanup handle was missing"
        })),
    }
}

pub fn enable(
    pid: i32,
    window_id: u32,
    descriptor: &'static BrowserSetupDescriptor,
) -> Result<SetupUiHandle, BrowserRefusal> {
    let initial = walk_tree(pid, Some(window_id), None);
    let initial_checkbox = exact_setup_checkbox(&initial.nodes, descriptor);
    let mut handle = match initial_checkbox {
        Ok(Some(_)) => SetupUiHandle {
            descriptor,
            close_button: None,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
        },
        Ok(None) => {
            let new_tab = new_tab_button(&initial.nodes, descriptor);
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
                    format!(
                        "{}'s exact New Tab action became stale before AXPress",
                        descriptor.product_name
                    ),
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            let (created, close_button) = loop {
                let created = walk_tree(pid, Some(window_id), None);
                match new_tab_close_button(&initial.nodes, &created.nodes, descriptor) {
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
                            format!(
                                "{} did not expose exactly one newly created tab",
                                descriptor.product_name
                            ),
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
                descriptor,
                close_button: Some(close_button),
                enable_attempted: false,
                trusted_checkbox_fallback_attempted: false,
                opened_setup_page: true,
                enabled_remote_debugging: false,
                focused_setup_address_field: false,
                foregrounded_window: false,
                injected_global_input: false,
            };
            let omnibox = unique_omnibox(&created.nodes, descriptor);
            let omnibox = match omnibox {
                Ok(element) => element,
                Err(error) => {
                    release_actionable_nodes(&created.nodes);
                    return Err(handle.abort(pid, window_id, error));
                }
            };
            let focused = unsafe { perform_action(omnibox as AXUIElementRef, "AXPress") };
            if focused != kAXErrorSuccess {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field became stale before AXPress",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            handle.focused_setup_address_field = true;
            let wrote_url = unsafe {
                set_string_attr(omnibox as AXUIElementRef, "AXValue", descriptor.setup_url)
            };
            if wrote_url != kAXErrorSuccess {
                release_actionable_nodes(&created.nodes);
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field rejected the fixed setup URL",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            let exact_value = unsafe { copy_string_attr(omnibox as AXUIElementRef, "AXValue") }
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url));
            let can_confirm = created
                .nodes
                .iter()
                .any(|node| node.element_ptr == omnibox && has_action(node, "AXConfirm"));
            let confirmed = can_confirm
                && unsafe { perform_action(omnibox as AXUIElementRef, "AXConfirm") }
                    == kAXErrorSuccess;
            unsafe { CFRetain(omnibox as CFTypeRef) };
            release_actionable_nodes(&created.nodes);
            if !exact_value {
                unsafe { CFRelease(omnibox as CFTypeRef) };
                return Err(handle.abort(
                    pid,
                    window_id,
                    refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{}'s exact address field did not retain the fixed setup URL",
                            descriptor.product_name
                        ),
                    ),
                ));
            }
            if confirmed {
                unsafe { CFRelease(omnibox as CFTypeRef) };
            } else {
                let deadline = Instant::now() + Duration::from_secs(2);
                let suggestion = loop {
                    let popup = walk_tree(pid, Some(window_id), None);
                    match exact_omnibox_suggestion(&popup.nodes, descriptor) {
                        Ok(Some(element)) => break Ok(Some((popup, element))),
                        Ok(None) if Instant::now() < deadline => {
                            release_actionable_nodes(&popup.nodes);
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Ok(None) => {
                            release_actionable_nodes(&popup.nodes);
                            break Ok(None);
                        }
                        Err(error) => {
                            release_actionable_nodes(&popup.nodes);
                            break Err(error);
                        }
                    }
                };
                match suggestion {
                    Ok(Some((popup, suggestion))) => {
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        let navigation =
                            unsafe { perform_action(suggestion as AXUIElementRef, "AXPress") };
                        release_actionable_nodes(&popup.nodes);
                        if navigation != kAXErrorSuccess {
                            return Err(handle.abort(
                                pid,
                                window_id,
                                refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    format!(
                                        "{}'s exact fixed-URL suggestion became stale before AXPress",
                                        descriptor.product_name
                                    ),
                                ),
                            ));
                        }
                    }
                    Ok(None) => {
                        handle.foregrounded_window = true;
                        handle.injected_global_input = true;
                        let navigated = crate::input::skylight::with_foreground_assist(
                            pid,
                            window_id,
                            || {
                                std::thread::sleep(Duration::from_millis(60));
                                if crate::apps::frontmost_pid() != Some(pid) {
                                    anyhow::bail!(
                                        "the approved browser lost foreground before setup navigation"
                                    );
                                }
                                let exact_value = unsafe {
                                    copy_string_attr(omnibox as AXUIElementRef, "AXValue")
                                }
                                .is_some_and(|value| {
                                    value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                                });
                                if !exact_value {
                                    anyhow::bail!(
                                        "the exact address field no longer contained the fixed setup URL"
                                    );
                                }
                                let focused = unsafe {
                                    perform_action(omnibox as AXUIElementRef, "AXPress")
                                };
                                if focused != kAXErrorSuccess {
                                    anyhow::bail!(
                                        "the exact address field became stale before setup navigation"
                                    );
                                }
                                let _ = unsafe {
                                    set_bool_attr_true(omnibox as AXUIElementRef, "AXFocused")
                                };
                                std::thread::sleep(Duration::from_millis(60));
                                let focused_element = unsafe { focused_element_of_pid(pid) };
                                let exact_focus = focused_element.is_some_and(|element| {
                                    let matches = is_same_element(element as usize, omnibox);
                                    unsafe { CFRelease(element as CFTypeRef) };
                                    matches
                                });
                                if !exact_focus {
                                    anyhow::bail!(
                                        "the exact address field did not own keyboard focus"
                                    );
                                }
                                crate::input::keyboard::press_key_global("a", &["cmd"])?;
                                std::thread::sleep(Duration::from_millis(30));
                                crate::input::keyboard::type_text(pid, descriptor.setup_url)?;
                                std::thread::sleep(Duration::from_millis(100));
                                let typed_exact_value = unsafe {
                                    copy_string_attr(omnibox as AXUIElementRef, "AXValue")
                                }
                                .is_some_and(|value| {
                                    value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                                });
                                if !typed_exact_value {
                                    anyhow::bail!(
                                        "trusted setup typing did not retain the fixed URL"
                                    );
                                }
                                crate::input::keyboard::press_key_global("return", &[])?;
                                std::thread::sleep(Duration::from_millis(100));
                                Ok(())
                            },
                        )
                        .and_then(|fronted| {
                            if fronted {
                                Ok(())
                            } else {
                                Err(anyhow::anyhow!(
                                    "the bounded setup foreground assist was unavailable"
                                ))
                            }
                        });
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        if let Err(error) = navigated {
                            return Err(handle.abort(
                                pid,
                                window_id,
                                refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    format!(
                                        "could not navigate the exact approved {} setup tab: {error}",
                                        descriptor.product_name
                                    ),
                                ),
                            ));
                        }
                    }
                    Err(error) => {
                        unsafe { CFRelease(omnibox as CFTypeRef) };
                        return Err(handle.abort(pid, window_id, error));
                    }
                }
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

    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree = walk_tree(pid, Some(window_id), None);
        let checkbox = exact_setup_checkbox(&tree.nodes, descriptor);
        match checkbox {
            Ok(Some(element)) => {
                let value = unsafe { copy_number_attr(element as AXUIElementRef, "AXValue") };
                match checkbox_state(value) {
                    Ok(CheckboxState::On) => {
                        if handle.enable_attempted {
                            handle.enabled_remote_debugging = true;
                        }
                        release_actionable_nodes(&tree.nodes);
                        return Ok(handle);
                    }
                    Ok(CheckboxState::Off) => {
                        if !handle.enable_attempted {
                            handle.enable_attempted = true;
                            let pressed =
                                unsafe { perform_action(element as AXUIElementRef, "AXPress") };
                            release_actionable_nodes(&tree.nodes);
                            if pressed != kAXErrorSuccess {
                                return Err(handle.abort(pid, window_id, refusal(
                                    BrowserRefusalCode::BrowserWrongTargetRefused,
                                    "the exact remote-debugging checkbox became stale before AXPress",
                                )));
                            }
                            continue;
                        }

                        if descriptor.product == BrowserProduct::MicrosoftEdge
                            && !handle.trusted_checkbox_fallback_attempted
                        {
                            let center =
                                unsafe { element_screen_center(element as AXUIElementRef) };
                            handle.trusted_checkbox_fallback_attempted = true;
                            release_actionable_nodes(&tree.nodes);
                            let Some((x, y)) = center else {
                                return Err(handle.abort(
                                    pid,
                                    window_id,
                                    refusal(
                                        BrowserRefusalCode::BrowserWrongTargetRefused,
                                        "the exact remote-debugging checkbox had no stable screen center",
                                    ),
                                ));
                            };
                            handle.foregrounded_window = true;
                            handle.injected_global_input = true;
                            let clicked = crate::input::skylight::with_foreground_assist(
                                pid,
                                window_id,
                                || {
                                    std::thread::sleep(Duration::from_millis(60));
                                    if crate::apps::frontmost_pid() != Some(pid) {
                                        anyhow::bail!(
                                            "the approved browser lost foreground before the setup click"
                                        );
                                    }
                                    crate::input::mouse::click_at_xy_desktop_preserving_cursor(x, y)?;
                                    std::thread::sleep(Duration::from_millis(60));
                                    Ok(())
                                },
                            )
                            .and_then(|fronted| {
                                if fronted {
                                    Ok(())
                                } else {
                                    Err(anyhow::anyhow!(
                                        "the bounded Microsoft Edge foreground assist was unavailable"
                                    ))
                                }
                            });
                            if let Err(error) = clicked {
                                return Err(handle.abort(
                                    pid,
                                    window_id,
                                    refusal(
                                        BrowserRefusalCode::BrowserWrongTargetRefused,
                                        format!(
                                            "could not toggle the exact Microsoft Edge remote-debugging checkbox: {error}"
                                        ),
                                    ),
                                ));
                            }
                            continue;
                        }

                        release_actionable_nodes(&tree.nodes);
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
                    format!(
                        "the exact {} remote-debugging setup page did not become ready",
                        descriptor.product_name
                    ),
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::{existing_profile_setup_descriptor, BrowserProduct};

    fn chrome() -> &'static BrowserSetupDescriptor {
        existing_profile_setup_descriptor(BrowserProduct::GoogleChrome).unwrap()
    }

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
            value_state: None,
            value_description: None,
            min_value: None,
            max_value: None,
            enabled: None,
            selected: None,
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
            node("AXWebArea", Some(chrome().page_titles[0]), None, &[]),
            node("AXHeading", Some(chrome().page_heading), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(chrome().setup_url),
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
        ];
        assert_eq!(exact_setup_checkbox(&nodes, chrome()).unwrap(), Some(7));

        let mut wrong_url = nodes.clone();
        wrong_url[2].value = Some("https://example.test/".to_owned());
        assert_eq!(exact_setup_checkbox(&wrong_url, chrome()).unwrap(), None);
    }

    #[test]
    fn checkbox_matcher_refuses_ambiguity() {
        let nodes = vec![
            node("AXWebArea", Some(chrome().page_titles[0]), None, &[]),
            node("AXHeading", Some(chrome().page_heading), None, &[]),
            node(
                "AXTextField",
                Some("Address and search bar"),
                Some(chrome().setup_url),
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
            node(
                "AXCheckBox",
                Some(chrome().checkbox_label),
                None,
                &["AXPress"],
            ),
        ];
        assert_eq!(
            exact_setup_checkbox(&nodes, chrome()).unwrap_err().code,
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
            select_new_tab_close_button(&before, &after, |left, right| left == right, chrome(),)
                .unwrap(),
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
            select_new_tab_close_button(&before, &after, |left, right| left == right, chrome(),)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn setup_navigation_selects_only_the_exact_omnibox_suggestion() {
        let omnibox = node(
            "AXTextField",
            Some("Address and search bar"),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        let mut popup = node("AXWebArea", Some("Omnibox Popup"), None, &[]);
        popup.depth = 1;
        let mut menu = node("AXMenu", None, None, &[]);
        menu.depth = 2;
        let mut exact = node(
            "AXMenuItem",
            Some(&format!(
                "{}, press Tab then Enter to Remove Suggestion.",
                chrome().setup_url
            )),
            None,
            &["AXPress"],
        );
        exact.element_ptr = 42;
        exact.depth = 3;
        let mut search = node(
            "AXMenuItem",
            Some("chrome://inspect/#remote-debugging search, Google Search"),
            None,
            &["AXPress"],
        );
        search.depth = 3;
        let mut outside = node(
            "AXMenuItem",
            Some(chrome().setup_url),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        outside.element_ptr = 99;
        outside.depth = 1;

        assert_eq!(
            exact_omnibox_suggestion(&[omnibox, popup, menu, exact, search, outside], chrome())
                .unwrap(),
            Some(42)
        );
    }

    #[test]
    fn setup_navigation_rejects_search_suggestion() {
        assert!(!is_exact_setup_suggestion(
            "chrome://inspect/#remote-debugging search, Google Search",
            chrome().setup_url
        ));
    }

    #[test]
    fn setup_navigation_refuses_multiple_exact_suggestions() {
        let omnibox = node(
            "AXTextField",
            Some("Address and search bar"),
            Some(chrome().setup_url),
            &["AXPress"],
        );
        let mut popup = node("AXWebArea", Some("Omnibox Popup"), None, &[]);
        popup.depth = 1;
        let mut first = node("AXMenuItem", Some(chrome().setup_url), None, &["AXPress"]);
        first.depth = 2;
        let mut second = first.clone();
        second.element_ptr = 8;

        assert_eq!(
            exact_omnibox_suggestion(&[omnibox, popup, first, second], chrome())
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
