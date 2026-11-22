// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A permission policy that constrains an action's target decides on what
//! the driver attests, and the action runs only against that same target.
//!
//! The policy is configured the way an operator does it, through
//! `CUA_DRIVER_POLICY_FILE`, which the driver reads once per process; this
//! test binary is that process. The platform is a fake desktop of
//! `list_windows` and `kill_app` tools whose state can change between the
//! policy decision and the dispatch.

#![cfg(feature = "yaml")]

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{
    ProtectedResourceScopeError, Tool, ToolDef, ToolRegistry, WindowOwnerResolver,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

const POLICY: &str = r#"
allow:
  rules:
    - tool: click
      target:
        executable: { allowed: [/apps/Editor] }
    - tool: type_text
      target:
        kind: { allowed: [window] }
        executable: { allowed: [/apps/Editor] }
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
    executable: &'static str,
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
                executable: "/apps/Editor",
            },
            Process {
                pid: TERMINAL,
                start_time: 1,
                executable: "/apps/Terminal",
            },
        ],
    }
}

/// The OS as the driver sees it: `before` until `switch_at` attestation
/// rounds have finished (the process fingerprint is a round's last read),
/// `after` from then on.
struct FakeDesktop {
    before: DesktopState,
    after: DesktopState,
    rounds: AtomicUsize,
    switch_at: AtomicUsize,
}

impl FakeDesktop {
    fn state(&self) -> &DesktopState {
        if self.rounds.load(Ordering::SeqCst) < self.switch_at.load(Ordering::SeqCst) {
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
        let scope = self
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
                        "executable": process.executable,
                    }
                })
            });
        self.0.rounds.fetch_add(1, Ordering::SeqCst);
        Ok(scope)
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::error("not used")
    }
}

/// Counts deliveries. With `hold`, each delivery waits for a permit, so the
/// action keeps the desktop input lane until the test releases it.
struct Action {
    hits: Arc<AtomicUsize>,
    hold: Option<Arc<tokio::sync::Semaphore>>,
    def: ToolDef,
}

#[async_trait::async_trait]
impl Tool for Action {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        self.hits.fetch_add(1, Ordering::SeqCst);
        if let Some(hold) = &self.hold {
            hold.acquire().await.unwrap().forget();
        }
        ToolResult::text("delivered")
    }
}

struct Fixture {
    registry: Arc<ToolRegistry>,
    desktop: Arc<FakeDesktop>,
    hits: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Options {
    hold: Option<Arc<tokio::sync::Semaphore>>,
    owner: Option<WindowOwnerResolver>,
    /// The `(pid, window_id)` every `element_token` belongs to.
    token_window: Option<(i32, u64)>,
    record_to: Option<std::path::PathBuf>,
}

fn fixture(before: DesktopState, after: DesktopState, options: Options) -> Fixture {
    configure_policy();
    let desktop = Arc::new(FakeDesktop {
        before,
        after,
        rounds: AtomicUsize::new(0),
        switch_at: AtomicUsize::new(1),
    });
    let hits = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ListWindows(
        desktop.clone(),
        def("list_windows", true),
    )));
    registry.register(Box::new(KillApp(desktop.clone(), def("kill_app", false))));
    for name in ["click", "type_text"] {
        registry.register(Box::new(Action {
            hits: hits.clone(),
            hold: options.hold.clone(),
            def: def(name, false),
        }));
    }
    if let Some(owner) = options.owner {
        registry.set_window_owner_resolver(owner);
    }
    if let Some(token_window) = options.token_window {
        registry.set_token_window_resolver(Arc::new(move |args| {
            args.get("element_token").map(|_| token_window)
        }));
    }
    if let Some(directory) = &options.record_to {
        registry
            .recording
            .start(directory.to_str().unwrap(), false, None)
            .unwrap();
    }
    Fixture {
        registry: Arc::new(registry),
        desktop,
        hits,
    }
}

fn registry(before: DesktopState, after: DesktopState) -> (Arc<ToolRegistry>, Arc<AtomicUsize>) {
    let fixture = fixture(before, after, Options::default());
    (fixture.registry, fixture.hits)
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
        format!("{result:?}").contains("'target.executable' is not in the allowed values"),
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
        format!("{result:?}").contains("target field 'executable' was not attested"),
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
async fn an_action_naming_only_a_pid_is_never_attested() {
    // Given a pid alone, the platform picks which of the process's windows
    // receives the action (the focused one, or on X11 one whose client
    // claims that pid), so no exact target can be attested for it.
    let (registry, hits) = registry(desktop(), desktop());
    for (tool, args) in [
        ("type_text", json!({"pid": EDITOR, "text": "hi"})),
        ("click", json!({"pid": EDITOR, "x": 1, "y": 2})),
    ] {
        let result = call(&registry, tool, args.clone()).await;
        assert_eq!(
            refusal_code(&result),
            Some("target_identity_unavailable"),
            "{args}: {result:?}"
        );
        assert!(
            format!("{result:?}").contains("a pid target must also name the window_id"),
            "{result:?}"
        );
    }
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
async fn selectors_that_could_name_different_targets_are_refused() {
    let (registry, hits) = registry(desktop(), desktop());
    // Without a pid, a desktop-frame call reaches the focused app on one
    // platform and the named window on another. Neither is attested.
    for args in [
        json!({"window_id": 7, "scope": "desktop", "text": "hi"}),
        json!({"window_id": 7, "text": "hi"}),
    ] {
        let result = call(&registry, "type_text", args.clone()).await;
        assert_eq!(
            refusal_code(&result),
            Some("target_identity_unavailable"),
            "{args}: {result:?}"
        );
        assert!(
            format!("{result:?}").contains("a window_id target must also name the window's pid"),
            "{result:?}"
        );
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_element_token_is_attested_as_its_snapshot_window_only_when_selectors_agree() {
    let fixture = fixture(
        desktop(),
        desktop(),
        Options {
            token_window: Some((EDITOR as i32, 7)),
            ..Options::default()
        },
    );
    let result = call(&fixture.registry, "click", json!({"element_token": "s:1"})).await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);

    for (args, detail) in [
        (
            json!({"element_token": "s:1", "scope": "desktop"}),
            "an element_token cannot be combined with a desktop frame and no pid",
        ),
        (
            json!({"element_token": "s:1", "pid": TERMINAL}),
            "the element_token belongs to pid 42 window 7, not the declared target",
        ),
    ] {
        let result = call(&fixture.registry, "click", args.clone()).await;
        assert_eq!(
            refusal_code(&result),
            Some("target_identity_unavailable"),
            "{args}: {result:?}"
        );
        assert!(format!("{result:?}").contains(detail), "{result:?}");
    }
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
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
        executable: "/apps/Editor",
    };
    let (registry, hits) = registry(desktop(), after);
    let result = call(
        &registry,
        "click",
        json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2}),
    )
    .await;
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

#[tokio::test]
async fn a_target_replaced_while_the_action_waits_for_the_input_lane_is_refused() {
    let mut after = desktop();
    after.processes[0].start_time = 2;
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    let fixture = fixture(
        desktop(),
        after,
        Options {
            hold: Some(hold.clone()),
            ..Options::default()
        },
    );
    fixture
        .desktop
        .switch_at
        .store(usize::MAX, Ordering::SeqCst);
    let args = json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2});

    // The first click is authorized, re-attested, and holds the desktop
    // input lane while it delivers.
    let first = tokio::spawn({
        let registry = fixture.registry.clone();
        let args = args.clone();
        async move { call(&registry, "click", args).await }
    });
    wait_until(|| fixture.hits.load(Ordering::SeqCst) == 1).await;
    let rounds = fixture.desktop.rounds.load(Ordering::SeqCst);

    // The second is authorized against the same window, then queues for
    // the lane. The process behind the window is replaced while it waits.
    let second = tokio::spawn({
        let registry = fixture.registry.clone();
        async move { call(&registry, "click", args).await }
    });
    wait_until(|| fixture.desktop.rounds.load(Ordering::SeqCst) > rounds).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.desktop.switch_at.store(0, Ordering::SeqCst);
    hold.add_permits(2);

    assert_ne!(first.await.unwrap().is_error, Some(true));
    let result = second.await.unwrap();
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_changed"),
        "{result:?}"
    );
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_owner_the_platform_verifies_overrides_a_claimed_one() {
    // list_windows says window 7 is the editor's (an X11 client can claim
    // any pid); the platform's verified owner says it is the terminal's.
    let fixture = fixture(
        desktop(),
        desktop(),
        Options {
            owner: Some(Arc::new(|_| Ok(TERMINAL))),
            ..Options::default()
        },
    );
    let result = call(
        &fixture.registry,
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
        format!("{result:?}").contains("window 7 belongs to pid 43, not pid 42"),
        "{result:?}"
    );
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_window_owner_the_platform_cannot_verify_is_refused() {
    let fixture = fixture(
        desktop(),
        desktop(),
        Options {
            owner: Some(Arc::new(|window_id| {
                Err(format!("owner of window {window_id} is unverifiable"))
            })),
            ..Options::default()
        },
    );
    let result = call(
        &fixture.registry,
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
        format!("{result:?}").contains("owner of window 7 is unverifiable"),
        "{result:?}"
    );
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 0);
}

/// The desktop whose process the recording screenshot hook replaces. The
/// hook is process-global, so only the recording test sets it.
static REPLACED_DURING_CAPTURE: Mutex<Option<Arc<FakeDesktop>>> = Mutex::new(None);

#[tokio::test]
async fn a_target_replaced_during_pre_action_recording_capture_is_refused() {
    cua_driver_core::recording::set_screenshot_fn(|_, _| {
        if let Some(desktop) = REPLACED_DURING_CAPTURE.lock().unwrap().as_ref() {
            desktop.switch_at.store(0, Ordering::SeqCst);
        }
        None
    });
    let mut after = desktop();
    after.processes[0].start_time = 2;
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture(
        desktop(),
        after,
        Options {
            record_to: Some(directory.path().to_owned()),
            ..Options::default()
        },
    );
    fixture
        .desktop
        .switch_at
        .store(usize::MAX, Ordering::SeqCst);
    *REPLACED_DURING_CAPTURE.lock().unwrap() = Some(fixture.desktop.clone());

    let result = call(
        &fixture.registry,
        "click",
        json!({"pid": EDITOR, "window_id": 7, "x": 1, "y": 2}),
    )
    .await;
    assert_eq!(
        refusal_code(&result),
        Some("target_identity_changed"),
        "{result:?}"
    );
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 0);
}

async fn wait_until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition reached");
}
