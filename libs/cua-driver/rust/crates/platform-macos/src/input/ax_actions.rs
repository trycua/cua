//! AX action dispatch — the preferred click/interaction path for indexed elements.

use crate::ax::bindings::*;
use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};

const MAX_SELECTION_ANCESTORS: usize = 8;

pub fn acknowledged(
    message: String,
    operation: &str,
    requested: cua_driver_core::action_record::RequestedDelivery,
    foreground: bool,
) -> cua_driver_core::protocol::ToolResult {
    use cua_driver_core::action_record::*;
    let record = ActionExecutionRecord::builder(
        ActionEffect::Unverifiable,
        ActionTransport::MacosAxAction,
        requested,
    )
    .actual_delivery(if foreground {
        ActualDelivery::Foreground
    } else {
        ActualDelivery::Background
    })
    .evidence(ActionEvidence {
        kind: EvidenceKind::NativeApiResult,
        detail: operation.into(),
    })
    .build()
    .expect("acknowledged native action");
    cua_driver_core::protocol::ToolResult::text(message).with_action_record(record)
}

fn is_selectable_container_role(role: &str) -> bool {
    matches!(role, "AXRow" | "AXCell" | "AXListItem" | "AXImage")
}

pub fn select_nearest_container(element_ptr: usize) -> anyhow::Result<Option<String>> {
    let Some(selection) = capture_nearest_container_selection(element_ptr) else {
        return Ok(None);
    };
    let settable = unsafe { is_attribute_settable_checked(selection.target, "AXSelected") }
        .map_err(|code| anyhow::anyhow!("selection capability read failed: {code}"))?;
    if !settable {
        return Ok(None);
    }
    let status = unsafe { set_bool_attr_true(selection.target, "AXSelected") };
    if status != kAXErrorSuccess || selection.observe().map(|state| state.0) != Some(true) {
        return Err(cua_driver_core::protocol::ToolResult::native_action_error(
            format!(
                "selection outcome is unknown ({status}); inspect fresh state and do not replay"
            ),
            cua_driver_core::action_record::ActionTransport::MacosAxValue,
        ));
    }
    Ok(Some(selection.role.clone()))
}

pub fn nearest_container_selection_state(element_ptr: usize) -> Option<(String, bool)> {
    let selection = capture_nearest_container_selection(element_ptr)?;
    Some((selection.role.clone(), selection.observe()?.0))
}

/// A retained selection context for proving the settled result of a modified
/// pointer click.
///
/// Reading only the target's `AXSelected` bit is insufficient for multi-select:
/// AppKit can expose a transient target transition while it is still resolving
/// the synthetic gesture, and a modifier-less outcome can replace the prior
/// selection with the target. Keep the target and every selected sibling alive
/// across delivery so callers can require both the intended target transition
/// and preservation of the pre-existing selection.
pub struct SelectionReadback {
    role: String,
    target: AXUIElementRef,
    peer_model_observed: bool,
    previously_selected_peers: Vec<AXUIElementRef>,
}

impl SelectionReadback {
    pub fn role(&self) -> &str {
        &self.role
    }

    pub fn observe(&self) -> Option<(bool, bool)> {
        let target_selected = unsafe { copy_bool_attr(self.target, "AXSelected") }?;
        let peers_preserved = self.peer_model_observed
            && self
                .previously_selected_peers
                .iter()
                .all(|&peer| unsafe { copy_bool_attr(peer, "AXSelected") } == Some(true));
        Some((target_selected, peers_preserved))
    }
}

impl Drop for SelectionReadback {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.target as CFTypeRef);
            for peer in self.previously_selected_peers.drain(..) {
                CFRelease(peer as CFTypeRef);
            }
        }
    }
}

/// Capture the nearest selectable container and its currently-selected peers.
/// Returns `None` when the platform does not expose a readable selection model;
/// callers must then leave the action unverifiable instead of inventing proof.
pub fn capture_nearest_container_selection(element_ptr: usize) -> Option<SelectionReadback> {
    let mut current = element_ptr as AXUIElementRef;
    let mut owns_current = false;

    for _ in 0..MAX_SELECTION_ANCESTORS {
        let role = unsafe { copy_string_attr(current, "AXRole") }.unwrap_or_default();
        if is_selectable_container_role(&role)
            && unsafe { copy_bool_attr(current, "AXSelected") }.is_some()
        {
            if !owns_current {
                unsafe { CFRetain(current as CFTypeRef) };
            }
            let target = current;
            let parent = unsafe { copy_element_attr(target, "AXParent") };
            let mut peers = Vec::new();
            let mut peer_model_observed = false;
            if let Some(parent) = parent {
                for child in unsafe { copy_children(parent) } {
                    let is_target =
                        unsafe { CFEqual(child as CFTypeRef, target as CFTypeRef) != 0 };
                    peer_model_observed |= is_target;
                    if !is_target && unsafe { copy_bool_attr(child, "AXSelected") } == Some(true) {
                        peers.push(child);
                    } else {
                        unsafe { CFRelease(child as CFTypeRef) };
                    }
                }
                unsafe { CFRelease(parent as CFTypeRef) };
            }
            return Some(SelectionReadback {
                role,
                target,
                peer_model_observed,
                previously_selected_peers: peers,
            });
        }

        let parent = unsafe { copy_element_attr(current, "AXParent") };
        if owns_current {
            unsafe { CFRelease(current as CFTypeRef) };
        }
        let Some(parent) = parent else {
            return None;
        };
        current = parent;
        owns_current = true;
    }

    if owns_current {
        unsafe { CFRelease(current as CFTypeRef) };
    }
    None
}

fn ensure_ax_enabled(enabled: Option<bool>, action: &str) -> anyhow::Result<()> {
    if enabled == Some(false) {
        anyhow::bail!(
            "refusing {action}: the target reports AXEnabled=false. \
             Retry this action with delivery_mode:\"foreground\" or call bring_to_front first"
        );
    }
    Ok(())
}

/// Refuse AX actions that macOS reports as disabled.
///
/// This must be checked immediately before dispatch rather than trusting the
/// cached snapshot value: foreground delivery can make a menu item live after
/// it was resolved, while backgrounding can disable it in the other direction.
pub fn ensure_ax_action_enabled(element_ptr: usize, action: &str) -> anyhow::Result<()> {
    let enabled = unsafe { copy_bool_attr(element_ptr as AXUIElementRef, "AXEnabled") };
    ensure_ax_enabled(enabled, action)
}

/// Perform an AX action on a freshly resolved element.
pub fn perform_ax_action(element_ptr: usize, action: &str) -> anyhow::Result<()> {
    let ax_action = map_action(action);
    ensure_ax_action_enabled(element_ptr, ax_action)?;
    let err = unsafe { perform_action(element_ptr as AXUIElementRef, ax_action) };

    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXUIElementPerformAction({action}) failed with error {err}")
    }
}

fn map_action(action: &str) -> &'static str {
    match action.to_lowercase().as_str() {
        "press" | "click" => "AXPress",
        "show_menu" | "right_click" | "rightclick" => "AXShowMenu",
        "pick" => "AXPick",
        "confirm" => "AXConfirm",
        "cancel" => "AXCancel",
        "open" => "AXOpen",
        _ => "AXPress",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_elements_are_refused_before_dispatch() {
        let error = ensure_ax_enabled(Some(false), "AXPick").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("AXEnabled=false"));
        assert!(message.contains("delivery_mode:\"foreground\""));
        assert!(message.contains("bring_to_front"));
    }

    #[test]
    fn enabled_or_unreported_state_is_allowed() {
        assert!(ensure_ax_enabled(Some(true), "AXPress").is_ok());
        assert!(ensure_ax_enabled(None, "AXPress").is_ok());
    }

    #[test]
    fn selection_fallback_is_limited_to_collection_item_roles() {
        for role in ["AXRow", "AXCell", "AXListItem", "AXImage"] {
            assert!(is_selectable_container_role(role), "{role}");
        }
        for role in ["AXButton", "AXTextField", "AXWindow", "AXOutline"] {
            assert!(!is_selectable_container_role(role), "{role}");
        }
    }
}

/// Set AXFocused=true on an element (for pre-focusing before key press).
pub fn focus_element(element_ptr: usize) -> anyhow::Result<()> {
    let err = unsafe { set_bool_attr_true(element_ptr as AXUIElementRef, "AXFocused") };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXSetAttribute(AXFocused) returned {err}; focus was not acknowledged")
    }
}

pub fn focus_target(pid: i32, element_ptr: usize) -> anyhow::Result<()> {
    cua_driver_core::tool::check_native_dispatch()?;
    if is_element_focused(pid, element_ptr) {
        return Ok(());
    }
    focus_element(element_ptr)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while !is_element_focused(pid, element_ptr) {
        cua_driver_core::tool::check_native_dispatch()?;
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("requested element did not become focused");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// Report whether `element_ptr` is the application's currently focused element.
///
/// This is a read-only confirmation for the foreground typing rung: an
/// `AXFocused` write can be accepted by the element and then immediately
/// clobbered when AppKit installs the window's remembered first responder, so
/// "the write returned success" is not evidence that focus stuck. Identity is
/// compared with `CFEqual` because the app hands back a fresh `AXUIElementRef`
/// for the same underlying element.
///
/// A `false` return is deliberately conservative: an app whose
/// `AXFocusedUIElement` is unreadable reports not-focused, which at worst costs
/// one extra re-apply.
pub fn is_element_focused(pid: i32, element_ptr: usize) -> bool {
    unsafe {
        let Some(focused) = crate::ax::bindings::focused_element_of_pid(pid) else {
            return false;
        };
        let same = CFEqual(focused as CFTypeRef, element_ptr as CFTypeRef) != 0;
        CFRelease(focused as CFTypeRef);
        same
    }
}

/// Set the AXValue of an element (for dropdowns, text fields, etc.).
pub fn set_ax_value(element_ptr: usize, value: &str) -> anyhow::Result<()> {
    let err = unsafe { set_string_attr(element_ptr as AXUIElementRef, "AXValue", value) };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
    }
}
