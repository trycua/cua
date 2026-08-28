//! Exact Windows UIA setup for Chromium existing-profile attachment.

use std::time::{Duration, Instant};
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};

use cua_driver_core::browser::{
    BrowserRefusal, BrowserRefusalCode, BrowserSetupDescriptor,
    EXISTING_PROFILE_SETUP_READY_TIMEOUT,
};
use windows::core::{Interface, BSTR};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationInvokePattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, ToggleState_Off, ToggleState_On, UIA_InvokePatternId,
    UIA_TogglePatternId, UIA_ValuePatternId,
};

use crate::uia::UiaNode;

// Native Chromium chrome is localized, so its accessible names are diagnostic
// text rather than a stable automation contract. Bootstrap against the exact
// approved HWND, native-vs-renderer boundary, control type, supported action,
// uniqueness, and exact post-action state instead of maintaining language
// allowlists or accepting fuzzy labels. The internal setup page follows the
// same rule: its native URL and web-control topology are contracts; its
// localized document, heading, and checkbox names are not.

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn release_nodes(nodes: &[UiaNode]) {
    for node in nodes.iter().filter(|node| node.element_ptr != 0) {
        unsafe { drop(IUIAutomationElement::from_raw(node.element_ptr as *mut _)) };
    }
}

fn unique_web_actionable(
    nodes: &[UiaNode],
    control_type: &str,
    action: &str,
) -> Result<Option<usize>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| {
            node.in_web_content
                && node.control_type == control_type
                && node.actions.iter().any(|value| value == action)
                && node.enabled != Some(false)
                && node.element_ptr != 0
        })
        .map(|node| node.element_ptr)
        .collect::<HashSet<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "multiple web {control_type} controls expose the exact {action} action on the setup page"
            ),
        )),
    }
}

fn unique_native_actionable(
    nodes: &[UiaNode],
    control_type: &str,
    action: &str,
) -> Result<Option<usize>, BrowserRefusal> {
    unique_native_actionable_with_focus(nodes, control_type, action, element_has_keyboard_focus)
}

fn element_has_keyboard_focus(element_ptr: usize) -> bool {
    if element_ptr == 0 {
        return false;
    }
    let element = unsafe { IUIAutomationElement::from_raw(element_ptr as *mut _) };
    let focused = unsafe { element.CurrentHasKeyboardFocus() }
        .ok()
        .is_some_and(|value| value.as_bool());
    std::mem::forget(element);
    focused
}

fn unique_native_actionable_with_focus(
    nodes: &[UiaNode],
    control_type: &str,
    action: &str,
    mut has_keyboard_focus: impl FnMut(usize) -> bool,
) -> Result<Option<usize>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| {
            !node.in_web_content
                && node.control_type == control_type
                && node.actions.iter().any(|value| value == action)
                && node.enabled != Some(false)
                && node.element_ptr != 0
        })
        .map(|node| node.element_ptr)
        .collect::<HashSet<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => {
            let focused = matches
                .into_iter()
                .filter(|element| has_keyboard_focus(*element))
                .collect::<Vec<_>>();
            match focused.as_slice() {
                [element] => Ok(Some(*element)),
                _ => Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!(
                        "multiple native {control_type} controls expose the exact {action} action, \
                         and keyboard focus did not identify exactly one"
                    ),
                )),
            }
        }
    }
}

fn native_tab_count(nodes: &[UiaNode]) -> usize {
    nodes
        .iter()
        .filter(|node| !node.in_web_content && node.control_type == "TabItem")
        .filter_map(|node| node.rect)
        .filter(|(left, top, right, bottom)| left < right && top < bottom)
        .collect::<std::collections::HashSet<_>>()
        .len()
}

fn exact_native_new_tab_button(nodes: &[UiaNode]) -> Result<Option<usize>, BrowserRefusal> {
    let Some(last_tab_index) = nodes.iter().rposition(|node| {
        !node.in_web_content
            && node.control_type == "TabItem"
            && node
                .rect
                .is_some_and(|(left, top, right, bottom)| left < right && top < bottom)
    }) else {
        return Ok(None);
    };
    let last_tab = &nodes[last_tab_index];
    let successor_index =
        (last_tab_index + 1..nodes.len()).find(|index| nodes[*index].depth <= last_tab.depth);
    let Some(successor) = successor_index.map(|index| &nodes[index]) else {
        return Ok(None);
    };
    let vertically_overlaps_tab_row = match (last_tab.rect, successor.rect) {
        (Some((_, tab_top, _, tab_bottom)), Some((_, button_top, _, button_bottom))) => {
            tab_top < button_bottom && button_top < tab_bottom
        }
        _ => false,
    };
    if successor.in_web_content
        || successor.control_type != "Button"
        || successor.enabled == Some(false)
        || successor.element_ptr == 0
        || !successor.actions.iter().any(|action| action == "invoke")
        || !vertically_overlaps_tab_row
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the native tab strip did not expose one exact structural new-tab action",
        ));
    }
    Ok(Some(successor.element_ptr))
}

fn stable_native_tab_count(hwnd: u64, initial_count: usize) -> Result<usize, BrowserRefusal> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut previous = initial_count;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let tree = crate::uia::walk_tree(hwnd, None);
        let current = native_tab_count(&tree.nodes);
        release_nodes(&tree.nodes);
        if current > 0 && current == previous {
            return Ok(current);
        }
        previous = current;
        if Instant::now() >= deadline {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved Chromium window did not expose a stable native tab topology",
            ));
        }
    }
}

fn setup_page_proven(nodes: &[UiaNode], descriptor: &BrowserSetupDescriptor) -> bool {
    let exact_url_count = nodes
        .iter()
        .filter(|node| {
            !node.in_web_content
                && node.control_type == "Edit"
                && node.actions.iter().any(|value| value == "set_value")
                && node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
        })
        .count();
    let document_count = nodes
        .iter()
        .filter(|node| node.control_type == "Document" && !node.in_web_content)
        .count();
    exact_url_count == 1 && document_count == 1
}

fn exact_setup_checkbox(
    nodes: &[UiaNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    if !setup_page_proven(nodes, descriptor) {
        return Ok(None);
    }
    unique_web_actionable(nodes, "CheckBox", "toggle")
}

unsafe fn set_value(element_ptr: usize, value: &str) -> Result<(), BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_ValuePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationValuePattern>())
        .and_then(|pattern| pattern.SetValue(&BSTR::from(value)));
    std::mem::forget(element);
    result.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("the exact UIA Value action failed: {error}"),
        )
    })
}

fn force_setup_foreground(target: windows::Win32::Foundation::HWND) -> (bool, bool) {
    unsafe { crate::input::force_foreground_assisted(target) }
}

unsafe fn invoke(element_ptr: usize, description: &str) -> Result<(), BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_InvokePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationInvokePattern>())
        .and_then(|pattern| pattern.Invoke());
    std::mem::forget(element);
    result.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("the exact {description} UIA Invoke action failed: {error}"),
        )
    })
}

fn confirm_setup_navigation(
    hwnd: u64,
    element_ptr: usize,
    foregrounded_window: &mut bool,
    injected_global_input: &mut bool,
    focused_setup_address_field: &mut bool,
) -> Result<(), BrowserRefusal> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let target = HWND(hwnd as *mut _);
    let prior = unsafe { GetForegroundWindow() };
    let navigation = (|| {
        let (fronted, injected) = force_setup_foreground(target);
        *injected_global_input |= injected;
        if !fronted {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "Windows refused the bounded foreground assist for the exact browser window",
            ));
        }
        *foregrounded_window = true;

        let element = unsafe { IUIAutomationElement::from_raw(element_ptr as *mut _) };
        let focused = unsafe {
            element
                .SetFocus()
                .and_then(|_| element.CurrentHasKeyboardFocus())
        };
        std::mem::forget(element);
        match focused {
            Ok(value) if value.as_bool() => *focused_setup_address_field = true,
            Ok(_) => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact address-and-search field did not acquire keyboard focus",
                ));
            }
            Err(error) => {
                return Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!("could not focus the exact address-and-search field: {error}"),
                ));
            }
        }

        *injected_global_input = true;
        crate::input::keyboard::send_key_synthesized(hwnd, "enter", &[]).map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("could not confirm the bounded setup navigation: {error}"),
            )
        })
    })();

    let restored = if prior.0.is_null() || prior == target {
        true
    } else {
        let (restored, injected) = force_setup_foreground(prior);
        *injected_global_input |= injected;
        restored
    };
    if !restored {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the setup navigation completed, but Windows refused to restore the prior foreground window",
        ));
    }
    navigation
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckboxState {
    Off,
    On,
}

unsafe fn checkbox_state(element_ptr: usize) -> Result<CheckboxState, BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_TogglePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationTogglePattern>())
        .and_then(|pattern| pattern.CurrentToggleState());
    std::mem::forget(element);
    match result {
        Ok(state) if state == ToggleState_Off => Ok(CheckboxState::Off),
        Ok(state) if state == ToggleState_On => Ok(CheckboxState::On),
        Ok(_) => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the exact remote-debugging checkbox had an indeterminate state",
        )),
        Err(error) => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("could not read the exact remote-debugging checkbox: {error}"),
        )),
    }
}

unsafe fn toggle(element_ptr: usize) -> Result<(), BrowserRefusal> {
    let element = IUIAutomationElement::from_raw(element_ptr as *mut _);
    let result = element
        .GetCurrentPattern(UIA_TogglePatternId)
        .and_then(|pattern| pattern.cast::<IUIAutomationTogglePattern>())
        .and_then(|pattern| pattern.Toggle());
    std::mem::forget(element);
    result.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("the exact UIA Toggle action failed: {error}"),
        )
    })
}

pub struct SetupUiHandle {
    hwnd: u64,
    descriptor: &'static BrowserSetupDescriptor,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
    enable_attempted: bool,
}

impl SetupUiHandle {
    fn rollback_remote_debugging(&mut self) -> bool {
        if !self.enabled_remote_debugging {
            return true;
        }
        let tree = crate::uia::walk_tree(self.hwnd, None);
        let checkbox = exact_setup_checkbox(&tree.nodes, self.descriptor);
        let restored = match checkbox {
            Ok(Some(element)) => unsafe {
                matches!(checkbox_state(element), Ok(CheckboxState::On)) && toggle(element).is_ok()
            },
            _ => false,
        };
        release_nodes(&tree.nodes);
        if restored {
            self.enabled_remote_debugging = false;
        }
        restored
    }

    pub fn abort(mut self, error: BrowserRefusal) -> BrowserRefusal {
        let enabled_remote_debugging = self.enabled_remote_debugging;
        let restored_remote_debugging = self.rollback_remote_debugging();
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

    pub fn close_for_success(mut self) -> Result<Option<bool>, BrowserRefusal> {
        if !self.opened_setup_page {
            return Ok(None);
        }
        let tree = crate::uia::walk_tree(self.hwnd, None);
        let proven = setup_page_proven(&tree.nodes, self.descriptor);
        release_nodes(&tree.nodes);
        if !proven {
            let error = refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the temporary setup page was no longer exact before cleanup",
            );
            return Err(self.abort(error));
        }
        if let Err(error) = crate::input::keyboard::send_key_synthesized(self.hwnd, "w", &["ctrl"])
        {
            let error = refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("could not close the exact temporary setup tab: {error}"),
            );
            return Err(self.abort(error));
        }
        self.opened_setup_page = false;
        Ok(Some(true))
    }

    pub fn close(self) -> Option<bool> {
        if !self.opened_setup_page {
            return None;
        }
        let tree = crate::uia::walk_tree(self.hwnd, None);
        let proven = setup_page_proven(&tree.nodes, self.descriptor);
        release_nodes(&tree.nodes);
        Some(
            proven
                && crate::input::keyboard::send_key_synthesized(self.hwnd, "w", &["ctrl"]).is_ok(),
        )
    }
}

fn pending_setups() -> &'static Mutex<HashMap<u64, SetupUiHandle>> {
    static PENDING: OnceLock<Mutex<HashMap<u64, SetupUiHandle>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn retain_pending(hwnd: u64, handle: SetupUiHandle) -> Result<(), BrowserRefusal> {
    let mut pending = pending_setups().lock().unwrap();
    if pending.contains_key(&hwnd) {
        drop(pending);
        return Err(handle.abort(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "another approved browser setup is already pending for this exact window",
        )));
    }
    pending.insert(hwnd, handle);
    Ok(())
}

pub fn commit_pending(hwnd: u64) -> Result<bool, BrowserRefusal> {
    let handle = pending_setups()
        .lock()
        .unwrap()
        .remove(&hwnd)
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "the exact pending browser setup cleanup handle is missing",
            )
        })?;
    Ok(handle.close_for_success()?.unwrap_or(false))
}

pub fn abort_pending(hwnd: u64, error: BrowserRefusal) -> BrowserRefusal {
    match pending_setups().lock().unwrap().remove(&hwnd) {
        Some(handle) => handle.abort(error),
        None => error.with_detail(serde_json::json!({
            "setup_cleanup": "the exact pending browser setup cleanup handle was missing"
        })),
    }
}

fn set_remote_debugging(
    hwnd: u64,
    descriptor: &'static BrowserSetupDescriptor,
    desired_enabled: bool,
) -> Result<SetupUiHandle, BrowserRefusal> {
    let initial = crate::uia::walk_tree(hwnd, None);
    let initial_checkbox = exact_setup_checkbox(&initial.nodes, descriptor);
    let mut handle = match initial_checkbox {
        Ok(Some(_)) => SetupUiHandle {
            hwnd,
            descriptor,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
            enable_attempted: false,
        },
        Ok(None) => {
            let initial_tab_count = native_tab_count(&initial.nodes);
            release_nodes(&initial.nodes);
            let tab_count_before = stable_native_tab_count(hwnd, initial_tab_count)?;

            let mut handle = SetupUiHandle {
                hwnd,
                descriptor,
                opened_setup_page: false,
                enabled_remote_debugging: false,
                focused_setup_address_field: false,
                foregrounded_window: false,
                injected_global_input: false,
                enable_attempted: false,
            };
            let tab_tree = crate::uia::walk_tree(hwnd, None);
            let new_tab_button = match exact_native_new_tab_button(&tab_tree.nodes) {
                Ok(Some(element)) => element,
                Ok(None) => {
                    release_nodes(&tab_tree.nodes);
                    return Err(handle.abort(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "the exact {} window has no structural native new-tab action",
                            descriptor.product_name
                        ),
                    )));
                }
                Err(error) => {
                    release_nodes(&tab_tree.nodes);
                    return Err(handle.abort(error));
                }
            };
            if let Err(error) = unsafe { invoke(new_tab_button, "native new-tab button") } {
                release_nodes(&tab_tree.nodes);
                return Err(handle.abort(error));
            }
            release_nodes(&tab_tree.nodes);
            handle.opened_setup_page = true;

            let deadline = Instant::now() + Duration::from_secs(3);
            let mut created = loop {
                let tree = crate::uia::walk_tree(hwnd, None);
                let tab_count_after = native_tab_count(&tree.nodes);
                if tab_count_after == tab_count_before + 1 {
                    break tree;
                }
                release_nodes(&tree.nodes);
                if Instant::now() >= deadline {
                    return Err(handle.abort(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "{} did not expose exactly one newly created tab",
                            descriptor.product_name
                        ),
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            let omnibox = match unique_native_actionable(&created.nodes, "Edit", "set_value") {
                Ok(Some(element)) => element,
                Ok(None) => {
                    release_nodes(&created.nodes);
                    return Err(handle.abort(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "the approved {} window has no unique native editable address field",
                            descriptor.product_name
                        ),
                    )));
                }
                Err(error) => {
                    release_nodes(&created.nodes);
                    return Err(handle.abort(error));
                }
            };
            if let Err(error) = unsafe { set_value(omnibox, descriptor.setup_url) } {
                release_nodes(&created.nodes);
                return Err(handle.abort(error));
            }
            release_nodes(&created.nodes);
            created = crate::uia::walk_tree(hwnd, None);
            let refreshed_omnibox =
                match unique_native_actionable(&created.nodes, "Edit", "set_value") {
                    Ok(Some(element))
                        if created.nodes.iter().any(|node| {
                            node.element_ptr == element
                                && node.value.as_deref().is_some_and(|value| {
                                    value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                                })
                        }) =>
                    {
                        element
                    }
                    Ok(_) => {
                        release_nodes(&created.nodes);
                        return Err(handle.abort(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            "the unique native address field did not retain the exact setup URL",
                        )));
                    }
                    Err(error) => {
                        release_nodes(&created.nodes);
                        return Err(handle.abort(error));
                    }
                };
            if let Err(error) = confirm_setup_navigation(
                hwnd,
                refreshed_omnibox,
                &mut handle.foregrounded_window,
                &mut handle.injected_global_input,
                &mut handle.focused_setup_address_field,
            ) {
                release_nodes(&created.nodes);
                return Err(handle.abort(error));
            }
            release_nodes(&created.nodes);
            handle
        }
        Err(error) => {
            release_nodes(&initial.nodes);
            return Err(error);
        }
    };
    if !handle.opened_setup_page {
        release_nodes(&initial.nodes);
    }

    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree = crate::uia::walk_tree(hwnd, None);
        let checkbox = exact_setup_checkbox(&tree.nodes, descriptor);
        match checkbox {
            Ok(Some(element)) => {
                let state = unsafe { checkbox_state(element) };
                let outcome = match state {
                    Ok(state) if (state == CheckboxState::On) == desired_enabled => {
                        if desired_enabled && handle.enable_attempted {
                            handle.enabled_remote_debugging = true;
                        } else if !desired_enabled {
                            handle.enabled_remote_debugging = false;
                        }
                        Ok(true)
                    }
                    Ok(_) if !handle.enable_attempted => unsafe { toggle(element).map(|_| false) },
                    Ok(_) => Ok(false),
                    Err(error) => Err(error),
                };
                release_nodes(&tree.nodes);
                match outcome {
                    Ok(true) => return Ok(handle),
                    Ok(false) => handle.enable_attempted = true,
                    Err(error) => return Err(handle.abort(error)),
                }
            }
            Ok(None) => release_nodes(&tree.nodes),
            Err(error) => {
                release_nodes(&tree.nodes);
                return Err(handle.abort(error));
            }
        }
        if Instant::now() >= deadline {
            return Err(handle.abort(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "the exact {} remote-debugging setup page did not become ready",
                    descriptor.product_name
                ),
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn enable(
    hwnd: u64,
    descriptor: &'static BrowserSetupDescriptor,
) -> Result<SetupUiHandle, BrowserRefusal> {
    set_remote_debugging(hwnd, descriptor, true)
}

pub fn disable(
    hwnd: u64,
    descriptor: &'static BrowserSetupDescriptor,
) -> Result<bool, BrowserRefusal> {
    let handle = set_remote_debugging(hwnd, descriptor, false)?;
    Ok(handle.close().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::{existing_profile_setup_descriptor, BrowserProduct};

    fn descriptor() -> &'static BrowserSetupDescriptor {
        existing_profile_setup_descriptor(BrowserProduct::GoogleChrome).unwrap()
    }

    fn node(control_type: &str, name: &str, value: Option<&str>, actions: &[&str]) -> UiaNode {
        UiaNode {
            element_index: (!actions.is_empty()).then_some(0),
            control_type: control_type.to_owned(),
            name: Some(name.to_owned()),
            value: value.map(str::to_owned),
            automation_id: None,
            help_text: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            enabled: None,
            selected: None,
            element_ptr: 7,
            center_x: 0,
            center_y: 0,
            rect: None,
            msaa_role: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    #[test]
    fn checkbox_requires_exact_internal_url_and_unique_web_toggle() {
        let mut checkbox = node("CheckBox", descriptor().checkbox_label, None, &["toggle"]);
        checkbox.in_web_content = true;
        let nodes = vec![
            node(
                "Edit",
                "Address and search bar",
                Some(descriptor().setup_url),
                &["set_value"],
            ),
            node("Document", descriptor().page_titles[0], None, &[]),
            node("Header", descriptor().page_heading, None, &[]),
            checkbox,
        ];
        assert_eq!(exact_setup_checkbox(&nodes, descriptor()).unwrap(), Some(7));

        let mut localized = nodes.clone();
        localized[0].name = Some("アドレス検索バー".to_owned());
        localized[1].name = Some("远程调试页面".to_owned());
        localized[2].name = Some("Удалённая отладка".to_owned());
        localized[3].name = Some("السماح بتصحيح الأخطاء لهذا المتصفح".to_owned());
        localized[3].in_web_content = true;
        assert_eq!(
            exact_setup_checkbox(&localized, descriptor()).unwrap(),
            Some(7)
        );

        let mut wrong_url = nodes.clone();
        wrong_url[0].value = Some("https://example.test/".to_owned());
        assert_eq!(
            exact_setup_checkbox(&wrong_url, descriptor()).unwrap(),
            None
        );
    }

    #[test]
    fn setup_page_names_are_opaque_across_unicode_scripts_and_normalization() {
        let samples = [
            ("e\u{301}", "é", "✅"),
            ("हिन्दी", "ไทย", "עברית"),
            ("日本語", "한국어", "简体中文"),
            ("\u{2067}العربية\u{2069}", "فارسی", "اردو"),
            ("Հայերեն", "ქართული", "አማርኛ"),
            ("👩🏽‍💻", "A\u{200d}B", "𐐷"),
            ("", "", ""),
        ];
        for (document_name, heading_name, checkbox_name) in samples {
            let mut checkbox = node("CheckBox", checkbox_name, None, &["toggle"]);
            checkbox.in_web_content = true;
            let nodes = vec![
                node(
                    "Edit",
                    "opaque native address field",
                    Some(descriptor().setup_url),
                    &["set_value"],
                ),
                node("Document", document_name, None, &[]),
                node("Header", heading_name, None, &[]),
                checkbox,
            ];
            assert_eq!(exact_setup_checkbox(&nodes, descriptor()).unwrap(), Some(7));
        }
    }

    #[test]
    fn setup_page_does_not_require_accessible_names() {
        let mut checkbox = node("CheckBox", "placeholder", None, &["toggle"]);
        checkbox.name = None;
        checkbox.in_web_content = true;
        let mut address = node(
            "Edit",
            "placeholder",
            Some(descriptor().setup_url),
            &["set_value"],
        );
        address.name = None;
        let mut document = node("Document", "placeholder", None, &[]);
        document.name = None;
        let nodes = vec![address, document, checkbox];

        assert_eq!(exact_setup_checkbox(&nodes, descriptor()).unwrap(), Some(7));
    }

    #[test]
    fn setup_page_refuses_ambiguous_web_toggles_without_reading_their_names() {
        let mut first = node("CheckBox", "A", None, &["toggle"]);
        first.element_ptr = 41;
        first.in_web_content = true;
        let mut second = node("CheckBox", "B", None, &["toggle"]);
        second.element_ptr = 42;
        second.in_web_content = true;
        let nodes = vec![
            node(
                "Edit",
                "address",
                Some(descriptor().setup_url),
                &["set_value"],
            ),
            node("Document", "opaque", None, &[]),
            first,
            second,
        ];

        assert_eq!(
            exact_setup_checkbox(&nodes, descriptor()).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn native_address_control_is_language_independent_without_trusting_web_content() {
        let mut native_address = node(
            "Edit",
            "アドレス検索バー",
            Some("chrome://inspect/#remote-debugging"),
            &["set_value"],
        );
        native_address.element_ptr = 11;
        let mut renderer_spoof = node(
            "Edit",
            "アドレス検索バー",
            Some("chrome://inspect/#remote-debugging"),
            &["set_value"],
        );
        renderer_spoof.element_ptr = 12;
        renderer_spoof.in_web_content = true;

        assert_eq!(
            unique_native_actionable(&[native_address, renderer_spoof], "Edit", "set_value")
                .unwrap(),
            Some(11)
        );
    }

    #[test]
    fn native_browser_controls_still_refuse_ambiguous_structural_matches() {
        let mut first = node("Edit", "Adress- und Suchleiste", None, &["set_value"]);
        first.element_ptr = 31;
        let mut second = node(
            "Edit",
            "Barre d'adresse et de recherche",
            None,
            &["set_value"],
        );
        second.element_ptr = 32;

        let error =
            unique_native_actionable_with_focus(&[first, second], "Edit", "set_value", |_| false)
                .unwrap_err();
        assert_eq!(error.code, BrowserRefusalCode::BrowserWrongTargetRefused);
    }

    #[test]
    fn native_address_control_uses_unique_keyboard_focus_when_edge_exposes_multiple_edits() {
        let mut first = node("Edit", "opaque first", None, &["set_value"]);
        first.element_ptr = 31;
        let mut second = node("Edit", "opaque second", None, &["set_value"]);
        second.element_ptr = 32;

        assert_eq!(
            unique_native_actionable_with_focus(&[first, second], "Edit", "set_value", |element| {
                element == 32
            },)
            .unwrap(),
            Some(32)
        );
    }

    #[test]
    fn native_address_control_refuses_multiple_focused_edits() {
        let mut first = node("Edit", "opaque first", None, &["set_value"]);
        first.element_ptr = 31;
        let mut second = node("Edit", "opaque second", None, &["set_value"]);
        second.element_ptr = 32;

        assert_eq!(
            unique_native_actionable_with_focus(&[first, second], "Edit", "set_value", |_| true,)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
    }

    #[test]
    fn setup_page_proof_rejects_duplicate_native_exact_urls() {
        let mut nodes = vec![
            node(
                "Edit",
                "Barra de direcciones y de búsqueda",
                Some(descriptor().setup_url),
                &["set_value"],
            ),
            node("Document", descriptor().page_titles[0], None, &[]),
            node("Header", descriptor().page_heading, None, &[]),
        ];
        let mut duplicate = nodes[0].clone();
        duplicate.element_ptr = 99;
        nodes.push(duplicate);

        assert!(!setup_page_proven(&nodes, descriptor()));
    }

    #[test]
    fn native_tab_count_is_structural_and_deduplicates_repeated_uia_rows() {
        let mut first = node("TabItem", "新标签页", None, &[]);
        first.rect = Some((10, 10, 110, 40));
        let duplicate = first.clone();
        let mut second = node("TabItem", "Neue Registerkarte", None, &[]);
        second.rect = Some((120, 10, 220, 40));
        let mut renderer_spoof = node("TabItem", "Tab", None, &[]);
        renderer_spoof.rect = Some((230, 10, 330, 40));
        renderer_spoof.in_web_content = true;
        let mut invalid = node("TabItem", "Onglet", None, &[]);
        invalid.rect = Some((0, 0, 0, 0));

        assert_eq!(
            native_tab_count(&[first, duplicate, second, renderer_spoof, invalid]),
            2
        );
    }

    #[test]
    fn native_new_tab_button_is_the_strict_successor_of_the_tab_strip() {
        let mut first = node("TabItem", "opaque-1", None, &["select"]);
        first.element_index = Some(10);
        first.element_ptr = 10;
        first.depth = 8;
        first.rect = Some((10, 10, 110, 40));
        let mut first_close = node("Button", "opaque-close-1", None, &["invoke"]);
        first_close.element_index = Some(11);
        first_close.element_ptr = 11;
        first_close.depth = 9;
        first_close.parent_element_index = Some(10);
        let mut second = node("TabItem", "opaque-2", None, &["select"]);
        second.element_index = Some(20);
        second.element_ptr = 20;
        second.depth = 8;
        second.rect = Some((120, 10, 220, 40));
        let mut second_close = node("Button", "opaque-close-2", None, &["invoke"]);
        second_close.element_index = Some(21);
        second_close.element_ptr = 21;
        second_close.depth = 9;
        second_close.parent_element_index = Some(20);
        let mut new_tab = node("Button", "opaque-new-tab", None, &["invoke"]);
        new_tab.element_ptr = 30;
        new_tab.depth = 6;
        new_tab.rect = Some((220, 10, 260, 40));

        assert_eq!(
            exact_native_new_tab_button(&[first, first_close, second, second_close, new_tab,])
                .unwrap(),
            Some(30)
        );
    }
}
