//! Fail-closed resolution for actions addressed only by process id.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::ToolResult;
use crate::tool::{ProtectedResourceOwnership, Tool, ToolDef};

/// Caller-recoverable metadata for an eligible top-level window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowTargetCandidate {
    pub window_id: u64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    pub is_on_screen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PidWindowTargetResolution {
    NotFound,
    Resolved(WindowTargetCandidate),
    Ambiguous(Vec<WindowTargetCandidate>),
}

/// Resolve the cardinality of eligible windows for one pid.
///
/// Duplicate ids are ignored defensively because some platforms merge more
/// than one native enumeration source.
pub fn resolve_pid_window_target(
    candidates: impl IntoIterator<Item = WindowTargetCandidate>,
) -> PidWindowTargetResolution {
    let mut seen = HashSet::new();
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.window_id))
        .collect();
    match candidates.as_slice() {
        [] => PidWindowTargetResolution::NotFound,
        [candidate] => PidWindowTargetResolution::Resolved(candidate.clone()),
        _ => PidWindowTargetResolution::Ambiguous(candidates),
    }
}

pub type WindowTargetCandidates =
    Arc<dyn Fn(i64) -> Vec<WindowTargetCandidate> + Send + Sync + 'static>;

/// Tool decorator that resolves PID-only calls before an action can send input.
/// Explicit `window_id` and `element_token` targets pass through unchanged.
pub struct PidOnlyWindowTargetGuard {
    inner: Box<dyn Tool>,
    candidates: WindowTargetCandidates,
}

impl PidOnlyWindowTargetGuard {
    pub fn new(inner: Box<dyn Tool>, candidates: WindowTargetCandidates) -> Self {
        Self { inner, candidates }
    }
}

#[async_trait]
impl Tool for PidOnlyWindowTargetGuard {
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

    async fn invoke(&self, mut args: Value) -> ToolResult {
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            || args.get("window_id").is_some_and(|value| !value.is_null())
            || args
                .get("element_token")
                .is_some_and(|value| !value.is_null())
        {
            return self.inner.invoke(args).await;
        }

        let Some(pid) = args.get("pid").and_then(Value::as_i64) else {
            return self.inner.invoke(args).await;
        };
        let candidates = self.candidates.clone();
        let candidates = match tokio::task::spawn_blocking(move || candidates(pid)).await {
            Ok(candidates) => candidates,
            Err(error) => {
                return ToolResult::error(format!(
                    "Could not enumerate eligible windows for pid {pid}: {error}"
                ))
                .with_structured(serde_json::json!({
                    "code": "window_target_resolution_failed",
                    "effect": "refused",
                    "pid": pid
                }))
            }
        };
        match resolve_pid_window_target(candidates) {
            PidWindowTargetResolution::NotFound => ToolResult::error(format!(
                "No eligible top-level windows found for pid {pid}."
            ))
            .with_structured(serde_json::json!({
                "code": "window_target_not_found",
                "effect": "refused",
                "pid": pid,
                "candidates": []
            })),
            PidWindowTargetResolution::Resolved(candidate) => {
                if let Some(object) = args.as_object_mut() {
                    object.insert("window_id".to_owned(), candidate.window_id.into());
                }
                self.inner.invoke(args).await
            }
            PidWindowTargetResolution::Ambiguous(candidates) => ToolResult::error(format!(
                "pid {pid} owns more than one eligible top-level window; provide window_id."
            ))
            .with_structured(serde_json::json!({
                "code": "ambiguous_window_target",
                "effect": "refused",
                "pid": pid,
                "candidates": candidates
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoTool {
        calls: Arc<AtomicUsize>,
    }

    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

    #[async_trait]
    impl Tool for EchoTool {
        fn def(&self) -> &ToolDef {
            DEF.get_or_init(|| ToolDef {
                name: "echo".into(),
                description: "test".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: false,
            })
        }

        async fn invoke(&self, args: Value) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::text("called").with_structured(args)
        }
    }

    fn candidate(window_id: u64) -> WindowTargetCandidate {
        WindowTargetCandidate {
            window_id,
            title: format!("Window {window_id}"),
            app_name: Some("Editor".into()),
            is_on_screen: true,
        }
    }

    #[test]
    fn resolver_reports_zero_one_and_many() {
        assert_eq!(
            resolve_pid_window_target([]),
            PidWindowTargetResolution::NotFound
        );
        assert_eq!(
            resolve_pid_window_target([candidate(7)]),
            PidWindowTargetResolution::Resolved(candidate(7))
        );
        assert_eq!(
            resolve_pid_window_target([candidate(7), candidate(8)]),
            PidWindowTargetResolution::Ambiguous(vec![candidate(7), candidate(8)])
        );
    }

    #[tokio::test]
    async fn ambiguity_refuses_before_invoking_action_with_recovery_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let guard = PidOnlyWindowTargetGuard::new(
            Box::new(EchoTool {
                calls: calls.clone(),
            }),
            Arc::new(|_| vec![candidate(7), candidate(8)]),
        );
        let result = guard.invoke(serde_json::json!({"pid": 42})).await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "ambiguous_window_target");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["candidates"][1]["window_id"], 8);
    }

    #[tokio::test]
    async fn unique_pid_target_is_promoted_to_explicit_window_id() {
        let calls = Arc::new(AtomicUsize::new(0));
        let guard = PidOnlyWindowTargetGuard::new(
            Box::new(EchoTool {
                calls: calls.clone(),
            }),
            Arc::new(|_| vec![candidate(7)]),
        );
        let result = guard.invoke(serde_json::json!({"pid": 42})).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.structured_content.unwrap()["window_id"], 7);
    }

    #[tokio::test]
    async fn explicit_window_and_element_token_targets_pass_through() {
        for args in [
            serde_json::json!({"pid": 42, "window_id": 9}),
            serde_json::json!({"pid": 42, "element_token": "explicit"}),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let guard = PidOnlyWindowTargetGuard::new(
                Box::new(EchoTool {
                    calls: calls.clone(),
                }),
                Arc::new(|_| panic!("explicit targets must not enumerate windows")),
            );
            let expected = args.clone();
            let result = guard.invoke(args).await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(result.structured_content.unwrap(), expected);
        }
    }
}
