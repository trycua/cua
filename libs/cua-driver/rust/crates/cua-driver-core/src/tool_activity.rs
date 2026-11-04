//! Per-call activity hook: the platform shell learns that an agent is
//! working each time the registry dispatches a tool.
//!
//! macOS uses it to keep the display awake while an agent works, because the
//! screen saver and display sleep put the session behind the lock screen,
//! where no app's windows are reachable. Registered once by the platform
//! crate; a no-op when nothing is registered.

use std::sync::OnceLock;

type ToolActivityFn = Box<dyn Fn() + Send + Sync>;
static TOOL_ACTIVITY_FN: OnceLock<ToolActivityFn> = OnceLock::new();

/// Register the activity callback. Returns false when one is already set;
/// the first registration wins.
pub fn set_tool_activity_fn(f: impl Fn() + Send + Sync + 'static) -> bool {
    TOOL_ACTIVITY_FN.set(Box::new(f)).is_ok()
}

/// Report one tool dispatch. Must stay cheap: it runs on every call.
pub fn note_tool_activity() {
    if let Some(f) = TOOL_ACTIVITY_FN.get() {
        f();
    }
}
