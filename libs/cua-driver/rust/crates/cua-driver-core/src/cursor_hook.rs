//! Cursor hook — a per-process callback fired whenever the agent cursor moves
//! or a press happens, so an embedder (for example a remote-desktop host) can
//! observe where every cursor is without polling or driving the overlay itself.
//!
//! Mirrors the `pip_hook` / `session` hook idiom: a single registered closure,
//! fired from the platform cursor-state write path. Coordinates are SCREEN
//! points (the space the overlay works in); the embedder maps them to whatever
//! window/target space it needs. No-op until an embedder registers a hook, so
//! there is zero cost in the common (daemon / CLI) case.

use std::sync::OnceLock;

/// One cursor update: the cursor identified by `cursor_id` is at screen point
/// (`x`, `y`), optionally in a pressed state (a click/drag press). Emitted on
/// every commanded move and on press edges.
#[derive(Debug, Clone)]
pub struct CursorHookEvent {
    pub cursor_id: String,
    pub x: f64,
    pub y: f64,
    pub pressed: bool,
}

type CursorHookFnBox = Box<dyn Fn(CursorHookEvent) + Send + Sync>;
static CURSOR_HOOK_FN: OnceLock<CursorHookFnBox> = OnceLock::new();

/// Register the process-wide cursor observer. Call once at startup.
pub fn set_cursor_hook_fn(f: impl Fn(CursorHookEvent) + Send + Sync + 'static) {
    let _ = CURSOR_HOOK_FN.set(Box::new(f));
}

/// True when an observer is registered (lets hot paths skip building an event).
pub fn cursor_hook_enabled() -> bool {
    CURSOR_HOOK_FN.get().is_some()
}

/// Fire a cursor update. No-op when nothing is registered.
pub fn push_cursor_event(event: CursorHookEvent) {
    if let Some(f) = CURSOR_HOOK_FN.get() {
        f(event);
    }
}
