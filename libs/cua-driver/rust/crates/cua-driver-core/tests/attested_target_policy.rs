// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A permission policy that constrains an action's target decides on what
//! the driver attests, and the action runs only against that same target.
//!
//! The policy is configured the way an operator does it, through
//! `CUA_DRIVER_POLICY_FILE`, which the driver reads once per process; this
//! test binary is that process. The platform is a fake desktop of
//! `list_windows`, `list_apps` and `kill_app` tools whose state can change
//! between the policy decision and the dispatch.

#![cfg(feature = "yaml")]

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{ProtectedResourceScopeError, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

const POLICY: &str = r#"
allow:
  rules:
    - tool: click
      target:
        bundle_id: { allowed: [com.example.Editor] }
    - tool: type_text
      target:
        kind: { allowed: [window] }
        bundle_id: { allowed: [com.example.Editor] }
"#;

const EDITOR: i64 = 42;
const TERMINAL: i64 = 43;

fn configure_policy() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join("policy.yaml");
        std::fs::write(&path, POLICY).unwrap();
        std::env::set_var("CUA_DRIVER_POLICY_FILE", &path);
    });
}

#[derive(Clone)]
struct Process {
    pid: i64,
    start_time: u64,
    bundle_id: &'static str,
}

#[derive(Clone)]
struct DesktopState {
    /// `(window_id, owner pid)`
    windows: Vec<(u64, i64)>,
    processes: Vec<Process>,
}

fn desktop() -> DesktopState {
    DesktopState {
        windows: vec![(7, EDITOR), (8, TERMINAL)],
        processes: vec![
            Process {
                pid: EDITOR,
                start_time: 1,
                bundle_id: "com.example.Editor",
            },
            Process {
                pid: TERMINAL,
                start_time: 1,
                bundle_id: "com.example.Terminal",
            },
        ],
    }
}

/// The OS as the driver sees it: `before` until the first attestation round
/// finishes (`list_apps` is its last read), `after` from then on.
struct FakeDesktop {
    before: DesktopState,
    after: DesktopState,
    rounds: AtomicUsize,
}

impl FakeDesktop {
    fn state(&self) -> &DesktopState {
        if self.rounds.load(Ordering::SeqCst) == 0 {
            &self.before
        } else {
            &self.after
        }
    }
}

fn def(name: &str, read_only: bool) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        description: "fake".to_owned(),
        input_schema: json!({"type": "object"}),
        read_only,
        destructive: false,
        idempotent: read_only,
        open_world: false,
    }
}

struct ListWindows(Arc<FakeDesktop>, ToolDef);

#[async_trait::async_trait]
impl Tool for ListWindows {
    fn def(&self) -> &ToolDef {
        &self.1
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let filter = args.get("pid").and_then(Value::as_i64);
        let windows: Vec<Value> = self
            .0
            .state()
            .windows
            .iter()
            .filter(|(_, pid)| filter.is_none_or(|filter| filter == *pid))
            .map(|(window_id, pid)| json!({"window_id": window_id, "pid": pid}))
            .collect();
        ToolResult::text("windows").with_structured(json!({ "windows": windows }))
    }
}

struct ListApps(Arc<FakeDesktop>, ToolDef);

#[async_trait::async_trait]
impl Tool for ListApps {
    fn def(&self) -> &ToolDef {
        &self.1
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let apps: Vec<Value> = self
            .0
            .state()
            .processes
            .iter()
            .map(|process| {
                json!({"pid": process.pid, "bundle_id": process.bundle_id, "name": process.bundle_id})
            })
            .collect();
        self.0.rounds.fetch_add(1, Ordering::SeqCst);
        ToolResult::text("apps").with_structured(json!({ "apps": apps }))
    }
}

struct KillApp(Arc<FakeDesktop>, ToolDef);

#[async_trait::async_trait]
impl Tool for KillApp {
    fn def(&self) -> &ToolDef {
        &self.1
    }

    async fn protected_resource_scope(
        &self,
        _adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, ProtectedResourceScopeError> {
        let pid = args.get("pid").and_then(Value::as_i64);
        Ok(self
            .0
            .state()
            .processes
            .iter()
            .find(|process| Some(process.pid) == pid)
            .map(|process| {
                json!({
                    "kind": "process_instance",
                    "fingerprint": {
                        "pid": process.pid,
                        "start_time": process.start_time,
                        "executable": format!("/apps/{}", process.bundle_id),
                    }
                })
            }))
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::error("not used")
    }
}

struct Action(Arc<AtomicUsize>, ToolDef);

#[async_trait::async_trait]
impl Tool for Action {
    fn def(&self) -> &ToolDef {
        &self.1
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        ToolResult::text("delivered")
    }
}

fn registry(before: DesktopState, after: DesktopState) -> (ToolRegistry, Arc<AtomicUsize>) {
    configure_policy();
    let desktop = Arc::new(FakeDesktop {
        before,
        after,
        rounds: AtomicUsize::new(0),
    });
    let hits = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ListWindows(
        desktop.clone(),
        def("list_windows", true),
    )));
    registry.register(Box::new(ListApps(desktop.clone(), def("list_apps", true))));
    registry.register(Box::new(KillApp(desktop, def("kill_app", false))));
    for name in ["click", "type_text"] {
        registry.register(Box::new(Action(hits.clone(), def(name, false))));
    }
    (registry, hits)
}

fn standard_context() -> Arc<EffectiveAuthorizationContext> {
    let ceiling = SessionModeCeiling::for_trusted_sessions(
        [PermissionMode::Standard],
        false,
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap();
    SessionAuthorizationRegistry::with_ceiling(ceiling)
        .compatibility_context(PermissionMode::Standard, None)
        .unwrap()
}

async fn call(registry: &ToolRegistry, tool: &str, args: Value) -> ToolResult {
    let mut args = args;
    args["session"] = json!("policy");
    registry
        .invoke_with_context(tool, args, standard_context())
        .await
}

fn refusal_code(result: &ToolResult) -> Option<&str> {
    let structured = result.structured_content.as_ref()?;
    structured.pointer("/refusal/code").and_then(Value::as_str)
}

#[tokio::test]
async fn a_window_of_the_allowed_app_runs() {
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(
        &registry,
        "click",
        json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2}),
    )
    .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_window_of_another_app_is_denied_by_its_attested_identity() {
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(
        &registry,
        "click",
        json!({"pid": TERMINAL, "window_id": 8, "x": 1, "y": 2}),
    )
    .await;
    assert_eq!(
        refusal_code(&result),
        Some("permission_denied"),
        "{result:?}"
    );
    assert!(
        format!("{result:?}").contains("'target.bundle_id' is not in the allowed values"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn declaring_the_allowed_pid_for_another_apps_window_is_refused() {
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(
        &registry,
        "click",
        json!({"pid": EDITOR, "window_id": 8, "x": 1, "y": 2}),
    )
    .await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_unavailable"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_desktop_action_has_no_app_identity_and_is_denied() {
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(&registry, "click", json!({"x": 1, "y": 2})).await;
    assert_eq!(
        refusal_code(&result),
        Some("permission_denied"),
        "{result:?}"
    );
    assert!(
        format!("{result:?}").contains("target field 'bundle_id' was not attested"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_element_token_without_a_known_snapshot_window_is_refused() {
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(&registry, "click", json!({"element_token": "s:1"})).await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_unavailable"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn keystrokes_a_policy_binds_to_one_window_cannot_follow_focus() {
    // A pid-only type_text lands in whichever of the app's windows is
    // focused when it runs. A policy that requires an exact window refuses
    // it rather than authorizing whatever window has focus at delivery.
    let (registry, hits) = registry(desktop(), desktop());
    let result = call(&registry, "type_text", json!({"pid": EDITOR, "text": "hi"})).await;
    assert_eq!(
        refusal_code(&result),
        Some("permission_denied"),
        "{result:?}"
    );
    assert!(
        format!("{result:?}").contains("'target.kind' is not in the allowed values"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let result = call(
        &registry,
        "type_text",
        json!({"pid": EDITOR, "window_id": 7, "text": "hi"}),
    )
    .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_window_handed_to_another_app_after_authorization_is_refused() {
    let mut after = desktop();
    after.windows = vec![(7, TERMINAL), (8, TERMINAL)];
    let (registry, hits) = registry(desktop(), after);
    let result = call(
        &registry,
        "click",
        json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2}),
    )
    .await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_unavailable"),
        "{result:?}"
    );
    assert!(
        format!("{result:?}").contains("window 7 is not an open window of pid 42"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_process_replaced_under_the_same_pid_after_authorization_is_refused() {
    let mut after = desktop();
    after.processes[0] = Process {
        pid: EDITOR,
        start_time: 2,
        bundle_id: "com.example.Editor",
    };
    let (registry, hits) = registry(desktop(), after);
    let result = call(&registry, "click", json!({"pid": EDITOR, "x": 1, "y": 2})).await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_changed"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_window_closed_after_authorization_is_refused() {
    let mut after = desktop();
    after.windows.retain(|(window_id, _)| *window_id != 7);
    let (registry, hits) = registry(desktop(), after);
    let result = call(
        &registry,
        "click",
        json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2}),
    )
    .await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_unavailable"),
        "{result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
