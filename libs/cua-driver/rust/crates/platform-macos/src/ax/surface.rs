//! Stateless, read-only observation of AX windows without CGWindowID.

use super::bindings::*;
use core_foundation::base::{CFRelease, CFTypeRef};
use serde::Serialize;

const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 2.0;

fn is_surface_candidate(
    role: &str,
    owner_pid: Option<i32>,
    requested_pid: i32,
    window_id: Option<u32>,
) -> bool {
    role == "AXWindow" && owner_pid == Some(requested_pid) && window_id.is_none()
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

pub fn observe_surfaces(pid: i32, max_elements: usize, max_depth: usize) -> ObservationSet {
    let mut result = ObservationSet {
        surfaces: Vec::new(),
        node_count: 0,
        truncated: false,
    };

    unsafe {
        let application = AXUIElementCreateApplication(pid);
        if application.is_null() {
            return result;
        }
        let _ = AXUIElementSetMessagingTimeout(application, AX_MESSAGING_TIMEOUT_SECONDS);
        let windows = copy_ax_windows(application);
        CFRelease(application as CFTypeRef);

        for window in windows {
            let _ = AXUIElementSetMessagingTimeout(window, AX_MESSAGING_TIMEOUT_SECONDS);
            let role = copy_string_attr(window, "AXRole").unwrap_or_default();
            let mut owner_pid = 0;
            let owner_pid =
                (AXUIElementGetPid(window, &mut owner_pid) == kAXErrorSuccess).then_some(owner_pid);
            if !is_surface_candidate(&role, owner_pid, pid, ax_get_window_id(window)) {
                CFRelease(window as CFTypeRef);
                continue;
            }

            let remaining = max_elements.saturating_sub(result.node_count);
            if remaining == 0 {
                result.truncated = true;
                CFRelease(window as CFTypeRef);
                continue;
            }
            let (nodes, truncated) = observe_tree(window, pid, remaining, max_depth);
            result.node_count += nodes.len();
            result.truncated |= truncated;
            result.surfaces.push(SurfaceObservation {
                kind: "ax_window",
                role,
                subrole: copy_string_attr(window, "AXSubrole"),
                title: nonempty_attr(window, "AXTitle"),
                identifier: nonempty_attr(window, "AXIdentifier"),
                frame: element_screen_rect(window),
                nodes,
                truncated,
            });
            CFRelease(window as CFTypeRef);
        }
    }
    result
}

unsafe fn nonempty_attr(element: AXUIElementRef, name: &str) -> Option<String> {
    copy_string_attr(element, name).filter(|value| !value.trim().is_empty())
}

unsafe fn observe_tree(
    root: AXUIElementRef,
    pid: i32,
    max_elements: usize,
    max_depth: usize,
) -> (Vec<ObservationNode>, bool) {
    let mut nodes = Vec::new();
    let mut truncated = false;
    walk_observation(
        root,
        pid,
        0,
        None,
        max_elements,
        max_depth,
        &mut nodes,
        &mut truncated,
    );
    (nodes, truncated)
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
) {
    if depth > max_depth || nodes.len() >= max_elements {
        *truncated = true;
        return;
    }
    let mut owner_pid = 0;
    if AXUIElementGetPid(element, &mut owner_pid) != kAXErrorSuccess || owner_pid != pid {
        return;
    }

    let role = copy_string_attr(element, "AXRole").unwrap_or_else(|| "AXUnknown".into());
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

    for child in copy_children(element) {
        walk_observation(
            child,
            pid,
            depth + 1,
            Some(index),
            max_elements,
            max_depth,
            nodes,
            truncated,
        );
        CFRelease(child as CFTypeRef);
    }
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
        assert!(is_surface_candidate("AXWindow", Some(7), 7, None));
        assert!(!is_surface_candidate("AXWindow", Some(8), 7, None));
        assert!(!is_surface_candidate("AXWindow", Some(7), 7, Some(42)));
        assert!(!is_surface_candidate("AXGroup", Some(7), 7, None));
        assert!(!is_surface_candidate("AXWindow", None, 7, None));
    }
}
