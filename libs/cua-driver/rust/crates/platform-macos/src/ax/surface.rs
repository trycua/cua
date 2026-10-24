//! Stateless, read-only observation of AX windows without CGWindowID.

use super::bindings::*;
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    string::CFString,
};
use serde::Serialize;

const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 2.0;

fn is_surface_candidate(
    role: &str,
    owner_pid: i32,
    requested_pid: i32,
    window_id: Option<u32>,
) -> bool {
    role == "AXWindow" && owner_pid == requested_pid && window_id.is_none()
}

#[derive(Debug, Clone, Serialize)]
pub struct ObservationFailure {
    pub operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribute: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_error: Option<AxErrorDetail>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxErrorDetail {
    pub code: AXError,
    pub name: &'static str,
}

impl ObservationFailure {
    fn ax(operation: &'static str, attribute: Option<&'static str>, code: AXError) -> Self {
        Self {
            operation,
            attribute,
            ax_error: Some(AxErrorDetail {
                code,
                name: ax_error_name(code),
            }),
        }
    }

    fn unavailable(operation: &'static str) -> Self {
        Self {
            operation,
            attribute: None,
            ax_error: None,
        }
    }
}

#[allow(non_upper_case_globals)]
fn ax_error_name(error: AXError) -> &'static str {
    match error {
        kAXErrorSuccess => "success",
        kAXErrorFailure => "failure",
        kAXErrorIllegalArgument => "illegal_argument",
        kAXErrorInvalidUIElement => "invalid_ui_element",
        kAXErrorInvalidUIElementObserver => "invalid_ui_element_observer",
        kAXErrorCannotComplete => "cannot_complete",
        kAXErrorAttributeUnsupported => "attribute_unsupported",
        kAXErrorActionUnsupported => "action_unsupported",
        kAXErrorNotificationUnsupported => "notification_unsupported",
        kAXErrorNotImplemented => "not_implemented",
        kAXErrorNotificationAlreadyRegistered => "notification_already_registered",
        kAXErrorNotificationNotRegistered => "notification_not_registered",
        kAXErrorAPIDisabled => "api_disabled",
        kAXErrorNoValue => "no_value",
        kAXErrorParameterizedAttributeUnsupported => "parameterized_attribute_unsupported",
        kAXErrorNotEnoughPrecision => "not_enough_precision",
        _ => "unknown",
    }
}

fn ax_ref(element: &CFType) -> AXUIElementRef {
    element.as_CFTypeRef() as AXUIElementRef
}

#[allow(non_upper_case_globals)]
unsafe fn attribute_count(
    element: AXUIElementRef,
    attribute: &'static str,
    unsupported_is_empty: bool,
) -> Result<usize, ObservationFailure> {
    let attribute_ref = CFString::new(attribute);
    let mut count = 0_isize;
    let error =
        AXUIElementGetAttributeValueCount(element, attribute_ref.as_concrete_TypeRef(), &mut count);
    match error {
        kAXErrorSuccess => Ok(count.max(0) as usize),
        kAXErrorNoValue => Ok(0),
        kAXErrorAttributeUnsupported if unsupported_is_empty => Ok(0),
        _ => Err(ObservationFailure::ax(
            "get_attribute_value_count",
            Some(attribute),
            error,
        )),
    }
}

unsafe fn copy_element_at(
    element: AXUIElementRef,
    attribute: &'static str,
    index: usize,
) -> Result<CFType, ObservationFailure> {
    let attribute_ref = CFString::new(attribute);
    let mut values: CFArrayRef = std::ptr::null();
    let error = AXUIElementCopyAttributeValues(
        element,
        attribute_ref.as_concrete_TypeRef(),
        index as isize,
        1,
        &mut values,
    );
    if error != kAXErrorSuccess {
        return Err(ObservationFailure::ax(
            "copy_attribute_values",
            Some(attribute),
            error,
        ));
    }
    if values.is_null() {
        return Err(ObservationFailure::unavailable("null_attribute_values"));
    }
    let array = CFArray::<CFType>::wrap_under_create_rule(values);
    let value = array
        .get(0)
        .map(|value| (*value).clone())
        .ok_or_else(|| ObservationFailure::unavailable("empty_attribute_values"))?;
    if value.type_of() != AXUIElementGetTypeID() {
        return Err(ObservationFailure::unavailable(
            "non_element_attribute_value",
        ));
    }
    Ok(value)
}

unsafe fn required_pid(
    element: AXUIElementRef,
    attribute: Option<&'static str>,
) -> Result<i32, ObservationFailure> {
    let mut pid = 0;
    let error = AXUIElementGetPid(element, &mut pid);
    if error == kAXErrorSuccess {
        Ok(pid)
    } else {
        Err(ObservationFailure::ax("get_pid", attribute, error))
    }
}

unsafe fn required_role(element: AXUIElementRef) -> Result<String, ObservationFailure> {
    let attribute = CFString::new("AXRole");
    let mut value: CFTypeRef = std::ptr::null();
    let error = AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value);
    if error != kAXErrorSuccess {
        return Err(ObservationFailure::ax(
            "copy_attribute_value",
            Some("AXRole"),
            error,
        ));
    }
    if value.is_null() {
        return Err(ObservationFailure::unavailable("invalid_ax_role"));
    }
    CFType::wrap_under_create_rule(value)
        .downcast_into::<CFString>()
        .map(|role| role.to_string())
        .ok_or_else(|| ObservationFailure::unavailable("invalid_ax_role"))
}

#[allow(non_upper_case_globals)]
unsafe fn window_id_or_absent(element: AXUIElementRef) -> Result<Option<u32>, ObservationFailure> {
    let mut window_id = 0_u32;
    let error = _AXUIElementGetWindow(element, &mut window_id);
    match error {
        kAXErrorSuccess => Ok((window_id != 0).then_some(window_id)),
        // A same-process NSAccessibilityElement published as AXWindow but not
        // backed by NSWindow returns illegal_argument here. That is the live
        // macOS representation of “this AXWindow has no CGWindowID”, not a
        // transport failure. Other errors retain their typed failure state.
        kAXErrorIllegalArgument | kAXErrorAttributeUnsupported | kAXErrorNoValue => Ok(None),
        _ => Err(ObservationFailure::ax("get_window_id", None, error)),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ObservationNode {
    pub observation_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_observation_index: Option<usize>,
    pub role: String,
    pub depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceObservation {
    pub kind: &'static str,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<[f64; 4]>,
    pub nodes: Vec<ObservationNode>,
    pub truncated: bool,
}

pub struct ObservationSet {
    pub surfaces: Vec<SurfaceObservation>,
    pub node_count: usize,
    pub truncated: bool,
}

pub fn observe_surfaces(
    pid: i32,
    max_elements: usize,
    max_depth: usize,
) -> Result<ObservationSet, ObservationFailure> {
    let mut result = ObservationSet {
        surfaces: Vec::new(),
        node_count: 0,
        truncated: false,
    };

    unsafe {
        let application_ref = AXUIElementCreateApplication(pid);
        if application_ref.is_null() {
            return Err(ObservationFailure::unavailable("create_application"));
        }
        let application = CFType::wrap_under_create_rule(application_ref as CFTypeRef);
        let timeout_error =
            AXUIElementSetMessagingTimeout(ax_ref(&application), AX_MESSAGING_TIMEOUT_SECONDS);
        if timeout_error != kAXErrorSuccess {
            return Err(ObservationFailure::ax(
                "set_messaging_timeout",
                None,
                timeout_error,
            ));
        }
        let window_count = attribute_count(ax_ref(&application), "AXWindows", false)?;

        for window_index in 0..window_count {
            let window = copy_element_at(ax_ref(&application), "AXWindows", window_index)?;
            let timeout_error =
                AXUIElementSetMessagingTimeout(ax_ref(&window), AX_MESSAGING_TIMEOUT_SECONDS);
            if timeout_error != kAXErrorSuccess {
                return Err(ObservationFailure::ax(
                    "set_messaging_timeout",
                    Some("AXWindows"),
                    timeout_error,
                ));
            }
            let role = required_role(ax_ref(&window))?;
            let owner_pid = required_pid(ax_ref(&window), Some("AXWindows"))?;
            let window_id = window_id_or_absent(ax_ref(&window))?;
            if !is_surface_candidate(&role, owner_pid, pid, window_id) {
                continue;
            }

            let remaining = max_elements.saturating_sub(result.node_count);
            if remaining == 0 {
                result.truncated = true;
                break;
            }
            let mut nodes = Vec::new();
            let mut truncated = false;
            walk_observation(
                ax_ref(&window),
                pid,
                0,
                None,
                remaining,
                max_depth,
                &mut nodes,
                &mut truncated,
            )?;
            result.node_count += nodes.len();
            result.truncated |= truncated;
            result.surfaces.push(SurfaceObservation {
                kind: "ax_window",
                role,
                subrole: copy_string_attr(ax_ref(&window), "AXSubrole"),
                title: nonempty_attr(ax_ref(&window), "AXTitle"),
                identifier: nonempty_attr(ax_ref(&window), "AXIdentifier"),
                frame: element_screen_rect(ax_ref(&window)),
                nodes,
                truncated,
            });
        }
    }
    Ok(result)
}

unsafe fn nonempty_attr(element: AXUIElementRef, name: &str) -> Option<String> {
    copy_string_attr(element, name).filter(|value| !value.trim().is_empty())
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_observation(
    element: AXUIElementRef,
    pid: i32,
    depth: usize,
    parent: Option<usize>,
    max_elements: usize,
    max_depth: usize,
    nodes: &mut Vec<ObservationNode>,
    truncated: &mut bool,
) -> Result<(), ObservationFailure> {
    if depth > max_depth || nodes.len() >= max_elements {
        *truncated = true;
        return Ok(());
    }
    if required_pid(element, Some("AXChildren"))? != pid {
        return Ok(());
    }

    let role = required_role(element)?;
    let title = nonempty_attr(element, "AXTitle");
    let description = nonempty_attr(element, "AXDescription");
    let value = copy_stringish_attr(element, "AXValue")
        .map(|value| value.state_value)
        .filter(|value| !value.trim().is_empty());
    let identifier = nonempty_attr(element, "AXIdentifier");
    let label = title
        .or(description)
        .or_else(|| value.clone())
        .or_else(|| identifier.clone());
    let index = nodes.len();
    nodes.push(ObservationNode {
        observation_index: index,
        parent_observation_index: parent,
        role,
        depth,
        label,
        value,
        identifier,
        frame: element_screen_rect(element),
        enabled: copy_bool_attr(element, "AXEnabled"),
        selected: copy_bool_attr(element, "AXSelected"),
    });

    let child_count = attribute_count(element, "AXChildren", true)?;
    if depth == max_depth && child_count > 0 {
        *truncated = true;
        return Ok(());
    }
    for child_index in 0..child_count {
        if nodes.len() >= max_elements {
            *truncated = true;
            break;
        }
        let child = copy_element_at(element, "AXChildren", child_index)?;
        walk_observation(
            ax_ref(&child),
            pid,
            depth + 1,
            Some(index),
            max_elements,
            max_depth,
            nodes,
            truncated,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_surface_has_no_native_or_action_identity() {
        let surface = SurfaceObservation {
            kind: "ax_window",
            role: "AXWindow".into(),
            subrole: None,
            title: Some("Document".into()),
            identifier: None,
            frame: None,
            nodes: Vec::new(),
            truncated: false,
        };
        let value = serde_json::to_value(surface).unwrap();
        assert!(value.get("window_id").is_none());
        assert!(value.get("surface_token").is_none());
        assert!(value.get("element_index").is_none());
        assert!(value.get("element_token").is_none());
    }

    #[test]
    fn discovery_selects_only_same_pid_ax_windows_without_cg_identity() {
        assert!(is_surface_candidate("AXWindow", 7, 7, None));
        assert!(!is_surface_candidate("AXWindow", 8, 7, None));
        assert!(!is_surface_candidate("AXWindow", 7, 7, Some(42)));
        assert!(!is_surface_candidate("AXGroup", 7, 7, None));
    }

    #[test]
    fn ax_errors_have_stable_typed_names() {
        let failure = ObservationFailure::ax(
            "copy_attribute_values",
            Some("AXWindows"),
            kAXErrorCannotComplete,
        );
        let value = serde_json::to_value(failure).unwrap();
        assert_eq!(value["operation"], "copy_attribute_values");
        assert_eq!(value["attribute"], "AXWindows");
        assert_eq!(value["ax_error"]["code"], kAXErrorCannotComplete);
        assert_eq!(value["ax_error"]["name"], "cannot_complete");
    }
}
