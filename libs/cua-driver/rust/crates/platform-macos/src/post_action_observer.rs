//! Target-scoped post-action accessibility-root observation.
//!
//! The decorator in this module is the only action topology producer on
//! macOS. It snapshots the addressed process, invokes the actuator, then
//! attaches one typed delta to `ToolResult`. Native notifications and global
//! window lists are not used as causality evidence.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::action_record::{
    resolve_surface_delta, ActionSurfaceCandidate, ActionSurfaceDelta, ActionSurfaceKind,
    ActionSurfaceTarget,
};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef};
use serde_json::Value;

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, copy_bool_attr, copy_element_array_attr, copy_string_attr,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};

const OBSERVATION_TIMEOUT: Duration = Duration::from_millis(1000);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RootKey {
    window_id: Option<u32>,
    parent_window_id: Option<u32>,
    role: String,
    subrole: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Root {
    target: Option<ActionSurfaceTarget>,
    focused: bool,
}

type RootSnapshot = HashMap<RootKey, Root>;

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

        let before = tokio::task::spawn_blocking(move || snapshot_roots(pid))
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
    before: RootSnapshot,
) -> Option<ActionSurfaceDelta> {
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    loop {
        let after = snapshot_roots(pid);
        let appeared: Vec<&Root> = after
            .iter()
            .filter(|(key, _)| !before.contains_key(*key))
            .map(|(_, root)| root)
            .collect();
        let foreground_changed = matches!(
            (prior_front, crate::apps::frontmost_pid()),
            (Some(before), Some(after)) if before != after
        );
        if !appeared.is_empty() || foreground_changed {
            let candidates = appeared
                .into_iter()
                .filter_map(|root| {
                    root.target.clone().map(|target| ActionSurfaceCandidate {
                        target,
                        focused: root.focused,
                    })
                })
                .collect();
            if let Some(delta) = resolve_surface_delta(candidates, foreground_changed) {
                return Some(delta);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn snapshot_roots(pid: i32) -> RootSnapshot {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return HashMap::new();
        }
        AXUIElementSetMessagingTimeout(app, 0.25);
        let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();
        let window_server = crate::windows::all_windows();
        let windows = copy_ax_windows(app);
        let mut roots = HashMap::new();
        for window in windows {
            let parent_window_id = ax_get_window_id(window);
            insert_root(
                &mut roots,
                window,
                parent_window_id,
                pid,
                &app_name,
                &window_server,
            );
            for child in copy_element_array_attr(window, "AXChildren") {
                let role = copy_string_attr(child, "AXRole").unwrap_or_default();
                if matches!(role.as_str(), "AXSheet" | "AXDialog" | "AXPopover") {
                    insert_root(
                        &mut roots,
                        child,
                        parent_window_id,
                        pid,
                        &app_name,
                        &window_server,
                    );
                }
                CFRelease(child as CFTypeRef);
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
    pid: i32,
    app_name: &str,
    window_server: &[crate::windows::WindowInfo],
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
    let kind = if role == "AXSheet" {
        ActionSurfaceKind::Sheet
    } else if role == "AXDialog" || subrole.to_ascii_lowercase().contains("dialog") {
        ActionSurfaceKind::Dialog
    } else if role == "AXPopover" || subrole.to_ascii_lowercase().contains("popover") {
        ActionSurfaceKind::Popover
    } else {
        ActionSurfaceKind::Window
    };
    let key = RootKey {
        window_id: own_window_id,
        parent_window_id,
        role,
        subrole,
    };
    let target = effective_window_id.and_then(|window_id| {
        surface_owner(window_server, pid, window_id, app_name).map(|(owner_pid, owner_app_name)| {
            ActionSurfaceTarget {
                pid: i64::from(owner_pid),
                window_id: u64::from(window_id),
                app_name: owner_app_name,
                title,
                kind,
                modal,
            }
        })
    });
    roots.insert(
        key,
        Root {
            target,
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

    fn root(window_id: u64, modal: bool, focused: bool) -> Root {
        let target = ActionSurfaceTarget {
            pid: 42,
            window_id,
            app_name: "Editor".into(),
            title: "Dialog".into(),
            kind: ActionSurfaceKind::Dialog,
            modal,
        };
        Root {
            target: Some(target),
            focused,
        }
    }

    #[test]
    fn exact_rebind_requires_one_validated_candidate() {
        let modal = root(7, true, false);
        let delta = resolve_surface_delta(
            vec![ActionSurfaceCandidate {
                target: modal.target.clone().expect("target"),
                focused: modal.focused,
            }],
            false,
        )
        .expect("delta");
        assert_eq!(
            delta.rebind.as_ref().map(|target| target.window_id),
            Some(7)
        );

        let focused = root(8, false, true);
        let delta = resolve_surface_delta(
            vec![modal, focused]
                .into_iter()
                .map(|root| ActionSurfaceCandidate {
                    target: root.target.expect("target"),
                    focused: root.focused,
                })
                .collect(),
            false,
        )
        .expect("delta");
        assert!(delta.rebind.is_none());
    }

    #[test]
    fn verified_panel_service_owner_becomes_the_rebind_pid() {
        let windows = vec![crate::windows::WindowInfo {
            window_id: 7,
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
        assert_eq!(
            surface_owner(&windows, 42, 7, "TextEdit"),
            Some((99, "Open and Save Panel Service".into()))
        );
        assert_eq!(surface_owner(&[], 42, 7, "TextEdit"), None);
    }

    #[test]
    fn unrelated_foreground_change_has_no_exact_target() {
        let delta = resolve_surface_delta(Vec::new(), true).expect("delta");
        assert!(delta.new_windows.is_empty());
        assert!(delta.rebind.is_none());
    }
}
