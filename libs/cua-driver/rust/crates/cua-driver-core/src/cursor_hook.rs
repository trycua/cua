//! Cursor hook — a per-process callback fired whenever the agent cursor moves
//! or a press happens, so an embedder (for example a remote-desktop host) can
//! observe where every cursor is without polling or driving the overlay itself.
//!
//! Mirrors the `pip_hook` / `session` hook idiom: a single registered closure,
//! fired from the platform cursor-state write path. Coordinates are SCREEN
//! points (the space the overlay works in); the embedder maps them to whatever
//! window/target space it needs. No-op until an embedder registers a hook, so
//! there is zero cost in the common (daemon / CLI) case.

use std::sync::atomic::{AtomicBool, Ordering};
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
///
/// Returns `true` if this call installed the observer, `false` if one was
/// already registered — in which case `f` is dropped and the existing observer
/// keeps receiving events.
///
/// Check the result. Registration is one-shot and there is no deregistration,
/// so a second caller silently gets nothing: a host that assumed it had the
/// cursor stream would render no pointers and have no way to find out why. If
/// this returns `false`, something else in the process owns the stream. Two
/// consumers in one process need a fan-out observer registered once, not two
/// calls here.
pub fn set_cursor_hook_fn(f: impl Fn(CursorHookEvent) + Send + Sync + 'static) -> bool {
    CURSOR_HOOK_FN.set(Box::new(f)).is_ok()
}

/// True when an observer is registered (lets hot paths skip building an event).
///
/// This answers "does anybody want these events", **not** "will any arrive".
/// See [`cursor_hook_supported`] for the latter.
pub fn cursor_hook_enabled() -> bool {
    CURSOR_HOOK_FN.get().is_some()
}

static EMITTER_DECLARED: AtomicBool = AtomicBool::new(false);

/// Declare that this platform adapter emits cursor events. Called once by an
/// adapter that actually drives [`push_cursor_event`] from its cursor write
/// path, during tool registration.
pub fn declare_cursor_hook_emitter() {
    EMITTER_DECLARED.store(true, Ordering::Release);
}

/// Whether this build emits cursor events at all.
///
/// Registering a hook always succeeds, so `cursor_hook_enabled()` cannot tell
/// an embedder whether events will ever arrive. Today only the macOS adapter
/// emits; the Windows, X11 and Wayland adapters have no cursor write path
/// wired to this hook, so on those hosts a registered hook stays silent
/// forever.
///
/// Per the cross-platform contract, a host must publish that limitation
/// explicitly — "cursor tracking unavailable on this platform" — rather than
/// showing a viewer a pointer that never moves and letting it look like a
/// hung stream. Check this at startup, not by waiting for a first event that
/// is not coming.
pub fn cursor_hook_supported() -> bool {
    EMITTER_DECLARED.load(Ordering::Acquire)
}

/// Fire a cursor update. No-op when nothing is registered.
pub fn push_cursor_event(event: CursorHookEvent) {
    if let Some(f) = CURSOR_HOOK_FN.get() {
        f(event);
    }
}
