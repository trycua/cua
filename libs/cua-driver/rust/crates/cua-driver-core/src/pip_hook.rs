//! Agent View event hook, registered once by `main.rs` when `--agent-view`
//! is enabled.
//!
//! The trait + factory live in the `pip-preview` crate so the platform
//! backends can implement them without depending on `cua-driver-core`.
//! What lives here is just the per-process callback that the tool
//! dispatcher uses to push frames after each successful tool call —
//! a thin shim so `tool.rs` doesn't need to know about `pip-preview`
//! directly and we keep the dependency graph one-directional.
//!
//! The PNG bytes pushed through here come from the existing
//! `SCREENSHOT_FN` callback (the same source `screenshot.png` uses in
//! the recording pipeline), so PiP shows exactly what the recorder
//! captures.

use std::sync::OnceLock;

/// Synthesized per-call frame payload. Kept structurally identical
/// to `pip_preview::PipFrame` — duplicated here to keep `cua-driver-core`
/// from importing `pip-preview` (the dependency would be circular once
/// platform backends pull both crates in).
#[derive(Clone, Copy)]
pub enum PipHookTargetKind {
    NativeWindow,
    BrowserTab,
}

pub struct PipHookTarget {
    pub workspace_id: String,
    pub workspace_label: String,
    pub target_id: String,
    pub identity_key: String,
    pub target_kind: PipHookTargetKind,
    pub target_label: String,
    pub native_container: Option<PipHookNativeContainer>,
}

#[derive(Clone, Copy)]
pub struct PipHookNativeContainer {
    pub pid: i64,
    pub window_id: u64,
}

pub struct PipHookFrame {
    pub target: PipHookTarget,
    pub png_bytes: Vec<u8>,
    pub action_label: String,
    pub timestamp_ms: u64,
    pub cursor_position: Option<(f64, f64)>,
}

pub enum PipHookEvent {
    Upsert(PipHookFrame),
    SetInputPassthrough {
        passthrough: bool,
    },
    RemoveTarget {
        workspace_id: String,
        identity_key: String,
    },
    RemoveWorkspace {
        workspace_id: String,
    },
}

/// Restores Agent View interactivity even when an action exits early or
/// unwinds. Physical desktop actions are serialized by the dispatcher, so one
/// process-global passthrough scope is sufficient.
pub struct PipInputPassthroughGuard;

impl Drop for PipInputPassthroughGuard {
    fn drop(&mut self) {
        if let Some(f) = PIP_EVENT_FN.get() {
            if let Err(error) = f(PipHookEvent::SetInputPassthrough { passthrough: false }) {
                tracing::error!(%error, "failed to restore Agent View input handling");
            }
        }
    }
}

type PipEventFnBox = Box<dyn Fn(PipHookEvent) -> Result<(), String> + Send + Sync>;
static PIP_EVENT_FN: OnceLock<PipEventFnBox> = OnceLock::new();

/// Register the platform-side push callback. `main.rs` calls this
/// once after starting the PiP backend.
pub fn set_pip_event_fn(f: impl Fn(PipHookEvent) -> Result<(), String> + Send + Sync + 'static) {
    let _ = PIP_EVENT_FN.set(Box::new(f));
}

/// True when a PiP backend is wired up. Tool dispatcher uses this to
/// skip the screenshot-bytes path when nothing would consume the
/// frame (avoiding wasted capture work when Agent View is disabled).
pub fn pip_enabled() -> bool {
    PIP_EVENT_FN.get().is_some()
}

/// Temporarily make Agent View input-transparent. The registered backend
/// applies the native state synchronously before this returns.
pub fn begin_pip_input_passthrough() -> Result<Option<PipInputPassthroughGuard>, String> {
    let Some(f) = PIP_EVENT_FN.get() else {
        return Ok(None);
    };
    f(PipHookEvent::SetInputPassthrough { passthrough: true })?;
    Ok(Some(PipInputPassthroughGuard))
}

/// Upsert one exact-target frame. No-op when no backend is registered.
pub fn push_pip_frame(frame: PipHookFrame) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::Upsert(frame));
    }
}

/// Remove one exact target card. Presentation only; never closes the target.
pub fn remove_pip_target(workspace_id: impl Into<String>, identity_key: impl Into<String>) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::RemoveTarget {
            workspace_id: workspace_id.into(),
            identity_key: identity_key.into(),
        });
    }
}

/// Remove presentation state for an ended workspace. This never closes the
/// workspace's native windows or browser tabs.
pub fn remove_pip_workspace(workspace_id: impl Into<String>) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::RemoveWorkspace {
            workspace_id: workspace_id.into(),
        });
    }
}
