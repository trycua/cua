//! Session lifecycle hooks.
//!
//! The cua-driver daemon (`serve.rs`) drives ONE shared `ToolRegistry`; every
//! `cua-driver mcp` proxy process connects to it and shares its state. A
//! proxy-minted `session_id` (carried in the daemon request envelope) lets the
//! daemon OWN and CLEAN UP per-session state.
//!
//! Recording ownership lives on the core `RecordingSession` directly. But some
//! session-scoped state is platform-specific (e.g. macOS per-session config
//! overrides in `platform-macos::tools::SessionConfigRegistry`) and the daemon
//! only holds an `Arc<ToolRegistry>` — it can't reach into a platform crate's
//! `ToolState`. This module bridges that gap with a small process-global list
//! of cleanup callbacks: each platform registers a `Fn(&str)` once at startup,
//! and the daemon's `session_end` arm fans the disconnecting `session_id` out
//! to all of them.
//!
//! This mirrors the existing screenshot/AX-snapshot callback pattern in
//! `recording.rs` — a registry-free, platform-pluggable hook set with no
//! reverse coupling from core into the platform crates.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

type SessionEndHook = Box<dyn Fn(&str) + Send + Sync>;

static SESSION_END_HOOKS: OnceLock<Mutex<Vec<SessionEndHook>>> = OnceLock::new();

/// Session ids that have already had their `session_end` fired. Dedupes the
/// control-connection EOF teardown (the reaper) against any stray legacy
/// `session_end` method that a mixed-version (new proxy / old proxy) rollout
/// might still send — `fire_session_end` is the single fan-out point and must
/// be idempotent because the overlay Remove + recording stop must run exactly
/// once. Growth is bounded (one short string per ended session over the
/// daemon's lifetime); eviction is a deliberate non-blocking follow-up.
static ENDED_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn hooks() -> &'static Mutex<Vec<SessionEndHook>> {
    SESSION_END_HOOKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn ended_sessions() -> &'static Mutex<HashSet<String>> {
    ENDED_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Register a callback invoked with the disconnecting `session_id` whenever a
/// session ends (graceful proxy EOF → daemon `session_end`). Each platform
/// registers its session-scoped cleanup here once at startup. Idempotency and
/// "unknown session id" tolerance are the hook's responsibility — `session_end`
/// fires once per proxy exit, but a hook should treat a clear of an unseen id
/// as a no-op.
pub fn register_session_end_hook(hook: impl Fn(&str) + Send + Sync + 'static) {
    hooks().lock().unwrap().push(Box::new(hook));
}

/// Fan a session-end out to every registered cleanup hook. Called by the daemon
/// on control-connection EOF (the reaper) and by the legacy `session_end` method
/// arm. Idempotent: the FIRST fire for a given `session_id` runs every hook; any
/// later fire for the same id is a no-op. This dedupes the EOF path against a
/// stray legacy `session_end` (mixed-version rollout) so cursor-remove +
/// recording-stop run exactly once. No-op when no hooks are registered.
pub fn fire_session_end(session_id: &str) {
    // Mark-then-fan-out under a short critical section, releasing the lock
    // before running hooks (hooks may be slow / re-entrant and must not hold
    // the dedupe lock).
    {
        let mut ended = ended_sessions().lock().unwrap();
        if !ended.insert(session_id.to_owned()) {
            return; // already ended — idempotent no-op.
        }
    }
    for hook in hooks().lock().unwrap().iter() {
        hook(session_id);
    }
}

/// Whether `fire_session_end` has already run for this `session_id`. The
/// daemon-side authority for "this session is permanently gone"; the macOS
/// overlay keeps its own render-side tombstone keyed on the same id.
pub fn is_session_ended(session_id: &str) -> bool {
    ended_sessions().lock().unwrap().contains(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn fire_session_end_is_idempotent_per_id() {
        // Distinct, test-local ids so we don't collide with other tests that
        // share the process-global ENDED_SESSIONS set.
        let sid = "test-dedupe-session-AABBCC";
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let want = sid.to_owned();
        register_session_end_hook(move |got| {
            if got == want {
                calls2.fetch_add(1, Ordering::Relaxed);
            }
        });

        assert!(!is_session_ended(sid));
        fire_session_end(sid);
        assert!(is_session_ended(sid));
        // Second + third fire for the same id must be no-ops.
        fire_session_end(sid);
        fire_session_end(sid);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "hook must run exactly once for a given session id"
        );
    }
}
