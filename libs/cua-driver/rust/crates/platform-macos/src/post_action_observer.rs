//! Target-scoped post-action accessibility-root observation.
//!
//! The decorator in this module is the only action topology producer on
//! macOS. It snapshots the addressed process, invokes the actuator, then
//! attaches one typed delta to `ToolResult`. Native notifications and global
//! window lists are not used as causality evidence.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::action_record::{
    resolve_surface_delta, ActionSurfaceCandidate, ActionSurfaceDelta, ActionSurfaceTarget,
};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef};
use serde_json::Value;

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, copy_bool_attr, copy_element_array_attr, copy_string_attr,
    element_screen_rect, AXUIElementCreateApplication, AXUIElementRef,
    AXUIElementSetMessagingTimeout,
};

const OBSERVATION_TIMEOUT: Duration = Duration::from_millis(400);
const POLL_INTERVAL: Duration = Duration::from_millis(30);
const CATCH_UP_INTERVAL: Duration = Duration::from_millis(80);
const CATCH_UP_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RootKey {
    Native {
        window_id: u32,
        role: String,
        subrole: String,
    },
    Transient {
        parent_window_id: Option<u32>,
        role: String,
        subrole: String,
        title: String,
        frame: Option<[i64; 4]>,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Root {
    window_id: Option<u32>,
    title: String,
    modal: bool,
    focused: bool,
}

type RootSnapshot = HashMap<RootKey, Root>;

#[derive(Default)]
struct RootObservation {
    roots: RootSnapshot,
    window_signature: HashSet<u32>,
}

pub struct ObservedActionTool {
    inner: Box<dyn Tool>,
}

impl ObservedActionTool {
    pub fn new(inner: Box<dyn Tool>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Tool for ObservedActionTool {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(pid) = args
            .get("pid")
            .and_then(Value::as_i64)
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        else {
            return self.inner.invoke(args).await;
        };
        let prior_front = crate::apps::frontmost_pid();
        let suppress_cross_app = args
            .get("delivery_mode")
            .and_then(Value::as_str)
            .is_none_or(|mode| !mode.eq_ignore_ascii_case("foreground"));
        let _suppression = prior_front
            .filter(|_| suppress_cross_app)
            .map(|restore_to| {
                crate::focus_steal::begin_suppression_allowing(
                    pid,
                    restore_to,
                    "ObservedActionTool",
                )
            });

        let before = tokio::task::spawn_blocking(move || begin_observation(pid))
            .await
            .unwrap_or_default();
        let mut result = self.inner.invoke(args).await;
        let delta = tokio::task::spawn_blocking(move || observe_delta(pid, prior_front, before))
            .await
            .ok()
            .flatten();
        if let Some(delta) = delta {
            result.surface_delta = Some(delta);
        }
        result
    }
}

fn observe_delta(
    pid: i32,
    prior_front: Option<i32>,
    before: RootObservation,
) -> Option<ActionSurfaceDelta> {
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let signaled = loop {
        if foreground_changed(prior_front)
            || target_window_signature(pid) != before.window_signature
        {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let mut appeared = appeared_roots(&before.roots, &snapshot_roots(pid));
    if appeared.is_empty() && signaled {
        for _ in 0..CATCH_UP_ATTEMPTS {
            std::thread::sleep(CATCH_UP_INTERVAL);
            appeared = appeared_roots(&before.roots, &snapshot_roots(pid));
            if !appeared.is_empty() {
                break;
            }
        }
    }
    resolve_appeared_roots(pid, &appeared, foreground_changed(prior_front))
}

fn foreground_changed(prior_front: Option<i32>) -> bool {
    matches!(
        (prior_front, crate::apps::frontmost_pid()),
        (Some(before), Some(after)) if before != after
    )
}

fn appeared_roots(before: &RootSnapshot, after: &RootSnapshot) -> Vec<Root> {
    after
        .iter()
        .filter(|(key, _)| !before.contains_key(*key))
        .map(|(_, root)| root.clone())
        .collect()
}

fn resolve_appeared_roots(
    pid: i32,
    roots: &[Root],
    foreground_changed: bool,
) -> Option<ActionSurfaceDelta> {
    if roots.is_empty() {
        return None;
    }
    let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();
    let mut resolved = resolve_candidates(pid, &app_name, roots, &crate::windows::all_windows());
    for _ in 0..CATCH_UP_ATTEMPTS {
        if resolved.len() == roots.len() {
            break;
        }
        std::thread::sleep(CATCH_UP_INTERVAL);
        resolved = resolve_candidates(pid, &app_name, roots, &crate::windows::all_windows());
    }
    let incomplete = resolved.len() != roots.len();
    let mut delta = resolve_surface_delta(resolved, foreground_changed)?;
    if incomplete {
        delta.rebind = None;
    }
    Some(delta)
}

fn resolve_candidates(
    pid: i32,
    app_name: &str,
    roots: &[Root],
    windows: &[crate::windows::WindowInfo],
) -> Vec<ActionSurfaceCandidate> {
    roots
        .iter()
        .filter_map(|root| {
            let window_id = root.window_id?;
            let (owner_pid, owner_app_name) = surface_owner(windows, pid, window_id, app_name)?;
            Some(ActionSurfaceCandidate {
                target: ActionSurfaceTarget {
                    pid: i64::from(owner_pid),
                    window_id: u64::from(window_id),
                    app_name: owner_app_name,
                    title: root.title.clone(),
                    modal: root.modal,
                },
                focused: root.focused,
            })
        })
        .collect()
}

fn begin_observation(pid: i32) -> RootObservation {
    RootObservation {
        roots: snapshot_roots(pid),
        window_signature: target_window_signature(pid),
    }
}

fn target_window_signature(pid: i32) -> HashSet<u32> {
    crate::windows::visible_windows()
        .into_iter()
        .filter(|window| window.pid == pid)
        .map(|window| window.window_id)
        .collect()
}

fn snapshot_roots(pid: i32) -> RootSnapshot {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return RootSnapshot::default();
        }
        AXUIElementSetMessagingTimeout(app, 0.25);
        let windows = copy_ax_windows(app);
        let mut roots = HashMap::new();
        for window in windows {
            let parent_window_id = ax_get_window_id(window);
            insert_root(&mut roots, window, parent_window_id);
            for attribute in ["AXSheets", "AXChildren"] {
                for child in copy_element_array_attr(window, attribute) {
                    let role = copy_string_attr(child, "AXRole").unwrap_or_default();
                    if matches!(role.as_str(), "AXSheet" | "AXDialog" | "AXPopover") {
                        insert_root(&mut roots, child, parent_window_id);
                    }
                    CFRelease(child as CFTypeRef);
                }
            }
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        roots
    }
}

unsafe fn insert_root(
    roots: &mut RootSnapshot,
    element: AXUIElementRef,
    parent_window_id: Option<u32>,
) {
    let role = copy_string_attr(element, "AXRole").unwrap_or_default();
    let subrole = copy_string_attr(element, "AXSubrole").unwrap_or_default();
    let title = copy_string_attr(element, "AXTitle").unwrap_or_default();
    let own_window_id = ax_get_window_id(element);
    let effective_window_id = own_window_id.or(parent_window_id);
    let modal = copy_bool_attr(element, "AXModal").unwrap_or(false)
        || role == "AXSheet"
        || role == "AXDialog"
        || subrole.to_ascii_lowercase().contains("dialog")
        || subrole.to_ascii_lowercase().contains("modal");
    let key = match own_window_id {
        Some(window_id) => RootKey::Native {
            window_id,
            role,
            subrole,
        },
        None => RootKey::Transient {
            parent_window_id,
            role,
            subrole,
            title: title.clone(),
            frame: element_screen_rect(element)
                .map(|frame| frame.map(|value| value.round() as i64)),
        },
    };
    roots.insert(
        key,
        Root {
            window_id: effective_window_id,
            title,
            modal,
            focused: copy_bool_attr(element, "AXFocused").unwrap_or(false),
        },
    );
}

fn surface_owner(
    windows: &[crate::windows::WindowInfo],
    target_pid: i32,
    window_id: u32,
    target_app_name: &str,
) -> Option<(i32, String)> {
    match crate::windows::resolve_window_owner_in(windows, target_pid, window_id) {
        crate::windows::WindowOwner::SamePid => Some((target_pid, target_app_name.to_owned())),
        crate::windows::WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => Some((owner_pid, owner_app_name)),
        crate::windows::WindowOwner::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(window_id: u32, title: &str, modal: bool) -> Root {
        Root {
            window_id: Some(window_id),
            title: title.into(),
            modal,
            focused: false,
        }
    }

    #[test]
    fn root_diff_ignores_metadata_changes_and_reports_an_appeared_modal() {
        let parent = RootKey::Native {
            window_id: 7,
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
        };
        let before = RootSnapshot::from([(parent.clone(), root(7, "Draft", false))]);
        let mut after = RootSnapshot::from([(parent, root(7, "Draft — Edited", false))]);
        assert!(appeared_roots(&before, &after).is_empty());

        let sheet = RootKey::Native {
            window_id: 8,
            role: "AXSheet".into(),
            subrole: String::new(),
        };
        after.insert(sheet, root(8, "Open", true));
        assert_eq!(appeared_roots(&before, &after), vec![root(8, "Open", true)]);
    }

    #[test]
    fn owner_resolution_can_follow_an_ax_root_without_guessing() {
        let roots = vec![root(8, "Open", true)];
        assert!(resolve_candidates(42, "TextEdit", &roots, &[]).is_empty());

        let windows = vec![crate::windows::WindowInfo {
            window_id: 8,
            pid: 99,
            app_name: "Open and Save Panel Service".into(),
            title: "Open".into(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 640.0,
                height: 480.0,
            },
            layer: 0,
            z_index: 1,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }];
        let candidates = resolve_candidates(42, "TextEdit", &roots, &windows);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].target.pid, 99);
        assert_eq!(candidates[0].target.window_id, 8);
    }

    #[test]
    fn unrelated_foreground_change_is_not_a_surface_delta() {
        assert_eq!(resolve_surface_delta(Vec::new(), true), None);
    }
}
