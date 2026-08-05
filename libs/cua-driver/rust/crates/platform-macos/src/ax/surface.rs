//! Bounded registry and read-only walker for AX windows without CGWindowID.

use super::bindings::*;
use core_foundation::base::{CFEqual, CFRelease, CFRetain, CFTypeRef};
use serde::Serialize;
use std::{collections::VecDeque, sync::Mutex};

const SURFACE_CAPACITY: usize = 64;
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
pub struct SurfaceDescriptor {
    pub surface_token: String,
    pub pid: i32,
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
}

struct SurfaceEntry {
    descriptor: SurfaceDescriptor,
    element_ptr: usize,
}

impl Drop for SurfaceEntry {
    fn drop(&mut self) {
        if self.element_ptr != 0 {
            unsafe { CFRelease(self.element_ptr as CFTypeRef) };
        }
    }
}

#[derive(Default)]
pub struct SurfaceRegistry {
    entries: Mutex<VecDeque<SurfaceEntry>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    Stale,
    ForeignPid,
    InvalidElement,
}

pub struct RetainedSurface(usize);

impl RetainedSurface {
    pub fn as_ptr(&self) -> AXUIElementRef {
        self.0 as AXUIElementRef
    }
}

impl Drop for RetainedSurface {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}

impl SurfaceRegistry {
    pub fn discover(&self, pid: i32) -> Vec<SurfaceDescriptor> {
        unsafe {
            let application = AXUIElementCreateApplication(pid);
            if application.is_null() {
                return Vec::new();
            }
            let _ = AXUIElementSetMessagingTimeout(application, AX_MESSAGING_TIMEOUT_SECONDS);
            super::enablement::ensure_chromium_ax_enabled(pid, application);
            let windows = copy_ax_windows(application);
            CFRelease(application as CFTypeRef);

            let mut surfaces = Vec::new();
            for window in windows {
                let _ = AXUIElementSetMessagingTimeout(window, AX_MESSAGING_TIMEOUT_SECONDS);
                let role = copy_string_attr(window, "AXRole").unwrap_or_default();
                let mut owner_pid = 0;
                let owner_pid = (AXUIElementGetPid(window, &mut owner_pid) == kAXErrorSuccess)
                    .then_some(owner_pid);
                if !is_surface_candidate(&role, owner_pid, pid, ax_get_window_id(window)) {
                    CFRelease(window as CFTypeRef);
                    continue;
                }
                surfaces.push(
                    self.insert_owned(
                        pid,
                        window,
                        role,
                        copy_string_attr(window, "AXSubrole"),
                        copy_string_attr(window, "AXTitle")
                            .filter(|value| !value.trim().is_empty()),
                        copy_string_attr(window, "AXIdentifier")
                            .filter(|value| !value.trim().is_empty()),
                        element_screen_rect(window),
                    ),
                );
            }
            surfaces
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_owned(
        &self,
        pid: i32,
        element: AXUIElementRef,
        role: String,
        subrole: Option<String>,
        title: Option<String>,
        identifier: Option<String>,
        frame: Option<[f64; 4]>,
    ) -> SurfaceDescriptor {
        let mut entries = self.entries.lock().unwrap();
        if let Some(existing) = entries.iter().find(|entry| {
            entry.descriptor.pid == pid
                && unsafe { CFEqual(entry.element_ptr as CFTypeRef, element as CFTypeRef) != 0 }
        }) {
            unsafe { CFRelease(element as CFTypeRef) };
            return existing.descriptor.clone();
        }

        let descriptor = SurfaceDescriptor {
            surface_token: format!("axw:{}", uuid::Uuid::new_v4()),
            pid,
            kind: "ax_window",
            role,
            subrole,
            title,
            identifier,
            frame,
        };
        entries.push_back(SurfaceEntry {
            descriptor: descriptor.clone(),
            element_ptr: element as usize,
        });
        while entries.len() > SURFACE_CAPACITY {
            entries.pop_front();
        }
        descriptor
    }

    pub fn resolve(&self, pid: i32, token: &str) -> Result<RetainedSurface, ResolveError> {
        let entries = self.entries.lock().unwrap();
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.descriptor.surface_token == token)
        else {
            return Err(ResolveError::Stale);
        };
        if entry.descriptor.pid != pid {
            return Err(ResolveError::ForeignPid);
        }
        let pointer = entry.element_ptr as AXUIElementRef;
        let mut owner_pid = 0;
        if unsafe { AXUIElementGetPid(pointer, &mut owner_pid) } != kAXErrorSuccess
            || owner_pid != pid
            || unsafe { copy_string_attr(pointer, "AXRole") }.as_deref() != Some("AXWindow")
            || unsafe { ax_get_window_id(pointer) }.is_some()
        {
            return Err(ResolveError::InvalidElement);
        }
        unsafe { CFRetain(pointer as CFTypeRef) };
        Ok(RetainedSurface(entry.element_ptr))
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

pub struct Observation {
    pub nodes: Vec<ObservationNode>,
    pub truncated: bool,
}

pub fn observe(
    surface: &RetainedSurface,
    pid: i32,
    max_elements: usize,
    max_depth: usize,
) -> Observation {
    let mut observation = Observation {
        nodes: Vec::new(),
        truncated: false,
    };
    unsafe {
        walk_observation(
            surface.as_ptr(),
            pid,
            0,
            None,
            max_elements,
            max_depth,
            &mut observation,
        );
    }
    observation
}

#[allow(clippy::too_many_arguments)]
unsafe fn walk_observation(
    element: AXUIElementRef,
    pid: i32,
    depth: usize,
    parent: Option<usize>,
    max_elements: usize,
    max_depth: usize,
    observation: &mut Observation,
) {
    if depth > max_depth {
        observation.truncated = true;
        return;
    }
    if observation.nodes.len() >= max_elements {
        observation.truncated = true;
        return;
    }
    let mut owner_pid = 0;
    if AXUIElementGetPid(element, &mut owner_pid) != kAXErrorSuccess || owner_pid != pid {
        return;
    }

    let role = copy_string_attr(element, "AXRole").unwrap_or_else(|| "AXUnknown".into());
    let title = copy_string_attr(element, "AXTitle").filter(|value| !value.trim().is_empty());
    let description =
        copy_string_attr(element, "AXDescription").filter(|value| !value.trim().is_empty());
    let value = copy_stringish_attr(element, "AXValue")
        .map(|value| value.state_value)
        .filter(|value| !value.trim().is_empty());
    let identifier =
        copy_string_attr(element, "AXIdentifier").filter(|value| !value.trim().is_empty());
    let label = title
        .clone()
        .or_else(|| description.clone())
        .or_else(|| value.clone())
        .or_else(|| identifier.clone());
    let index = observation.nodes.len();
    observation.nodes.push(ObservationNode {
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
            observation,
        );
        CFRelease(child as CFTypeRef);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_descriptor_never_exposes_native_or_action_identity() {
        let descriptor = SurfaceDescriptor {
            surface_token: "axw:test".into(),
            pid: 7,
            kind: "ax_window",
            role: "AXWindow".into(),
            subrole: None,
            title: Some("Document".into()),
            identifier: None,
            frame: None,
        };
        let value = serde_json::to_value(descriptor).unwrap();
        assert!(value.get("surface_token").is_some());
        assert!(value.get("window_id").is_none());
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
