//! D-MAC-7 regression: recording must not multiply action latency.
//!
//! The platform application-state hook is process-global, so this file runs
//! in its own test binary with a deliberately slow fake accessibility
//! provider. A walk that ignored its budget used to hold every recorded action
//! for the full provider latency (33 s per click on a Tk window), including
//! clicks refused before any input was dispatched.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::recording::{
    set_budgeted_ax_snapshot_fn, set_screenshot_fn, StateCaptureBudget,
};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// Simulated provider latency when the hook ignores its budget.
const SLOW_PROVIDER: Duration = Duration::from_secs(6);
const BUDGET_MS: u64 = 100;
/// pid whose provider ignores the budget entirely.
const SLOW_PID: i64 = 4101;
/// pid whose provider honours the budget and reports a partial tree.
const BUDGETED_PID: i64 = 4102;

static STATE_CALLS: AtomicUsize = AtomicUsize::new(0);

fn install_hooks() {
    let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();
    set_screenshot_fn(move |_, _| Some(png.clone()));
    set_budgeted_ax_snapshot_fn(|window_id, pid, budget: StateCaptureBudget| {
        STATE_CALLS.fetch_add(1, Ordering::SeqCst);
        match pid {
            Some(SLOW_PID) => {
                std::thread::sleep(SLOW_PROVIDER);
                Some(br#"{"tree_markdown":"late"}"#.to_vec())
            }
            _ => serde_json::to_vec(&json!({
                "pid": pid,
                "window_id": window_id,
                "element_count": 1,
                "tree_markdown": "- AXWindow",
                "truncated": true,
                "truncation_reason": "timeout",
                "nodes_visited": 1,
                "nodes_pending": 7,
                "walk_elapsed_ms": budget.timeout_ms,
                "timeout_ms": budget.timeout_ms,
            }))
            .ok(),
        }
    });
}

/// Refuses capture-bound calls the way every platform adapter does, and
/// otherwise reports an immediate unverifiable pixel click.
struct FakeClick {
    def: ToolDef,
}

#[async_trait]
impl Tool for FakeClick {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if args.get("capture_id").is_some() {
            return ToolResult::error(
                "capture binding failed: capture id is unknown. Not dispatching click.",
            )
            .with_structured(json!({"code": "capture_not_found", "effect": "refused"}));
        }
        ToolResult::text("✅ Posted click (background CGEvent; not driver-verified).")
    }
}

fn registry(output_dir: &std::path::Path) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeClick {
        def: ToolDef {
            name: "click".into(),
            description: "fake click".into(),
            input_schema: json!({"type": "object"}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
    }));
    registry
        .recording
        .start_with_state_budget(
            output_dir.to_str().unwrap(),
            false,
            None,
            Some(StateCaptureBudget {
                timeout_ms: BUDGET_MS,
            }),
        )
        .unwrap();
    registry
}

fn evidence(turn: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(turn.join("evidence.json")).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn recorded_actions_stay_bounded_with_a_slow_accessibility_provider() {
    install_hooks();
    let directory = tempfile::tempdir().unwrap();
    let registry = registry(directory.path());
    let backstop = StateCaptureBudget {
        timeout_ms: BUDGET_MS,
    }
    .backstop();

    // 1. A click refused before dispatch walks nothing and returns promptly.
    let started = Instant::now();
    let refused = registry
        .invoke(
            "click",
            json!({
                "pid": SLOW_PID,
                "window_id": 7,
                "x": 1,
                "y": 1,
                "capture_id": "capture_edd825b82cc0441896ba7e888cd36d94_0000000000000016",
            }),
        )
        .await;
    let refused_elapsed = started.elapsed();
    assert_eq!(refused.is_error, Some(true));
    assert!(
        refused_elapsed < Duration::from_millis(800),
        "refused click took {refused_elapsed:?}"
    );
    assert_eq!(
        STATE_CALLS.load(Ordering::SeqCst),
        0,
        "no state walk for a refusal"
    );
    let turn = directory.path().join("turn-00001");
    let manifest = evidence(&turn);
    for phase in ["before", "after"] {
        assert_eq!(
            manifest[phase]["state"],
            json!({"status": "not_applicable", "classification": "action_refused_before_dispatch"}),
            "{phase}: {manifest}"
        );
        assert_eq!(manifest[phase]["screenshot"]["status"], "captured");
        assert!(!turn.join(format!("{phase}_state.json")).exists());
    }

    // 2. A dispatched click against a provider that ignores its budget is
    //    held for at most one backstop, not for the provider's latency.
    let started = Instant::now();
    let dispatched = registry
        .invoke(
            "click",
            json!({"pid": SLOW_PID, "window_id": 7, "x": 1, "y": 1}),
        )
        .await;
    let dispatched_elapsed = started.elapsed();
    assert!(
        dispatched_elapsed < backstop + Duration::from_millis(900),
        "recorded click took {dispatched_elapsed:?} (backstop {backstop:?}); result {dispatched:?}"
    );
    assert!(dispatched_elapsed < SLOW_PROVIDER);
    let manifest = evidence(&directory.path().join("turn-00002"));
    assert_eq!(
        manifest["before"]["state"],
        json!({"status": "unavailable", "classification": "state_capture_timeout"})
    );
    // The abandoned walk is still running, so the after phase does not start
    // a second walk of the same hung application.
    assert_eq!(
        manifest["after"]["state"],
        json!({"status": "unavailable", "classification": "state_capture_busy"})
    );
    assert_eq!(STATE_CALLS.load(Ordering::SeqCst), 1);

    // 3. A provider that honours its budget receives it and its partial tree
    //    is recorded as truncated evidence.
    let started = Instant::now();
    registry
        .invoke(
            "click",
            json!({"pid": BUDGETED_PID, "window_id": 8, "x": 1, "y": 1}),
        )
        .await;
    assert!(started.elapsed() < backstop);
    let turn = directory.path().join("turn-00003");
    let manifest = evidence(&turn);
    for phase in ["before", "after"] {
        let state = &manifest[phase]["state"];
        assert_eq!(state["status"], "captured", "{phase}: {manifest}");
        assert_eq!(state["truncated"], true);
        assert_eq!(state["truncation_reason"], "timeout");
        assert_eq!(state["nodes_pending"], 7);
        assert_eq!(state["timeout_ms"], BUDGET_MS);
        assert!(turn.join(format!("{phase}_state.json")).is_file());
    }

    registry.recording.stop_owner(None).unwrap();
}
