//! Exact Windows UIA setup for Chromium existing-profile attachment.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
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

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn field_equals(node: &UiaNode, expected: &str) -> bool {
    [
        node.name.as_deref(),
        node.value.as_deref(),
        node.automation_id.as_deref(),
        node.help_text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn release_nodes(nodes: &[UiaNode]) {
    for node in nodes.iter().filter(|node| node.element_ptr != 0) {
        unsafe { drop(IUIAutomationElement::from_raw(node.element_ptr as *mut _)) };
    }
}

fn unique_actionable(
    nodes: &[UiaNode],
    control_type: &str,
    label: &str,
    action: &str,
) -> Result<Option<usize>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| {
            node.control_type == control_type
                && field_equals(node, label)
                && node.actions.iter().any(|value| value == action)
        })
        .map(|node| node.element_ptr)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [element] => Ok(Some(*element)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!("multiple exact {control_type} controls matched {label:?}"),
        )),
    }
}

fn setup_page_proven(nodes: &[UiaNode], descriptor: &BrowserSetupDescriptor) -> bool {
    let exact_url = nodes.iter().any(|node| {
        node.control_type == "Edit"
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
    });
    let exact_heading = nodes.iter().any(|node| {
        matches!(node.control_type.as_str(), "Header" | "Text")
            && field_equals(node, descriptor.page_heading)
    });
    let exact_page = nodes.iter().any(|node| {
        node.control_type == "Document"
            && descriptor
                .page_titles
                .iter()
                .any(|title| field_equals(node, title))
    });
    exact_url && exact_page && exact_heading
}

fn exact_setup_checkbox(
    nodes: &[UiaNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<usize>, BrowserRefusal> {
    if !setup_page_proven(nodes, descriptor) {
        return Ok(None);
    }
    unique_actionable(nodes, "CheckBox", descriptor.checkbox_label, "toggle")
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
            format!("the exact UIA Invoke action failed: {error}"),
        )
    })
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
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };

    if unsafe { crate::input::force_foreground_attached(target) } {
        return (true, false);
    }

    // The approved setup transition is allowed to be visible. Claim the
    // foreground-lock token with Windows' reserved no-name key, which has no
    // application action, then retry the exact HWND a bounded number of times.
    const VK_NONAME: u8 = 0xFC;
    unsafe {
        keybd_event(VK_NONAME, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_NONAME, 0, KEYEVENTF_KEYUP, 0);
    }
    for _ in 0..3 {
        if unsafe { crate::input::force_foreground_attached(target) } {
            return (true, true);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    (false, true)
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

pub fn enable(
    hwnd: u64,
    descriptor: &'static BrowserSetupDescriptor,
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
            let tab_count_before = initial
                .nodes
                .iter()
                .filter(|node| node.control_type == "TabItem")
                .count();
            let new_tab = match unique_actionable(&initial.nodes, "Button", "New Tab", "invoke") {
                Ok(Some(element)) => element,
                Ok(None) => {
                    release_nodes(&initial.nodes);
                    return Err(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "the approved {} window has no exact New Tab button",
                            descriptor.product_name
                        ),
                    ));
                }
                Err(error) => {
                    release_nodes(&initial.nodes);
                    return Err(error);
                }
            };
            let invoked = unsafe { invoke(new_tab) };
            release_nodes(&initial.nodes);
            invoked?;

            let mut handle = SetupUiHandle {
                hwnd,
                descriptor,
                opened_setup_page: true,
                enabled_remote_debugging: false,
                focused_setup_address_field: false,
                foregrounded_window: false,
                injected_global_input: false,
                enable_attempted: false,
            };

            let deadline = Instant::now() + Duration::from_secs(3);
            let mut created = loop {
                let tree = crate::uia::walk_tree(hwnd, None);
                let tab_count_after = tree
                    .nodes
                    .iter()
                    .filter(|node| node.control_type == "TabItem")
                    .count();
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
            let omnibox = unique_actionable(
                &created.nodes,
                "Edit",
                "Address and search bar",
                "set_value",
            )?
            .ok_or_else(|| {
                refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!(
                        "the approved {} window has no exact address-and-search field",
                        descriptor.product_name
                    ),
                )
            });
            let omnibox = match omnibox {
                Ok(element) => element,
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
            let refreshed_omnibox = created
                .nodes
                .iter()
                .find(|node| {
                    node.control_type == "Edit"
                        && field_equals(node, "Address and search bar")
                        && node.value.as_deref().is_some_and(|value| {
                            value.trim().eq_ignore_ascii_case(descriptor.setup_url)
                        })
                })
                .map(|node| node.element_ptr);
            let Some(refreshed_omnibox) = refreshed_omnibox else {
                release_nodes(&created.nodes);
                return Err(handle.abort(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the exact address field did not retain the setup URL",
                )));
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
                    Ok(CheckboxState::On) => {
                        if handle.enable_attempted {
                            handle.enabled_remote_debugging = true;
                        }
                        Ok(true)
                    }
                    Ok(CheckboxState::Off) if !handle.enable_attempted => unsafe {
                        toggle(element).map(|_| false)
                    },
                    Ok(CheckboxState::Off) => Ok(false),
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
    fn checkbox_requires_exact_url_heading_and_unique_toggle() {
        let nodes = vec![
            node(
                "Edit",
                "Address and search bar",
                Some(descriptor().setup_url),
                &["set_value"],
            ),
            node("Document", descriptor().page_titles[0], None, &[]),
            node("Header", descriptor().page_heading, None, &[]),
            node("CheckBox", descriptor().checkbox_label, None, &["toggle"]),
        ];
        assert_eq!(exact_setup_checkbox(&nodes, descriptor()).unwrap(), Some(7));

        let mut wrong_url = nodes.clone();
        wrong_url[0].value = Some("https://example.test/".to_owned());
        assert_eq!(
            exact_setup_checkbox(&wrong_url, descriptor()).unwrap(),
            None
        );
    }
}
