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

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

type SessionEndHook = Box<dyn Fn(&str) + Send + Sync>;
type SessionReviveHook = Box<dyn Fn(&str) + Send + Sync>;

static SESSION_END_HOOKS: OnceLock<Mutex<Vec<SessionEndHook>>> = OnceLock::new();
static SESSION_REVIVE_HOOKS: OnceLock<Mutex<Vec<SessionReviveHook>>> = OnceLock::new();

/// Last-activity timestamp per live session id. A session is "touched" every
/// time a tool call carries its explicit `session` id (see the daemon boundary
/// in `serve.rs`). The idle-TTL sweep ([`evict_idle`]) ends sessions that
/// haven't been touched within the TTL — this is the cleanup path that replaces
/// connection-EOF reaping now that a session is a caller-declared identity, not
/// a per-MCP-connection one. `"default"` and empty ids are never tracked (they
/// are the anonymous, cursor-less fallback).
static SESSION_ACTIVITY: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn activity() -> &'static Mutex<HashMap<String, Instant>> {
    SESSION_ACTIVITY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether `id` is a real, trackable session id (not the anonymous fallback).
fn is_trackable(id: &str) -> bool {
    !id.is_empty() && id != "default"
}

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

fn revive_hooks() -> &'static Mutex<Vec<SessionReviveHook>> {
    SESSION_REVIVE_HOOKS.get_or_init(|| Mutex::new(Vec::new()))
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

/// Register a callback for an explicit reuse of an ended session id. Platform
/// cursor overlays use this to order a `Revive` event after their prior
/// `Remove`, preserving the late-command guard without making the tombstone
/// permanent.
pub fn register_session_revive_hook(hook: impl Fn(&str) + Send + Sync + 'static) {
    revive_hooks().lock().unwrap().push(Box::new(hook));
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

/// Whether `fire_session_end` has already run for this `session_id`. This is
/// the daemon-side late-action guard until an explicit [`revive_session`] call;
/// platform overlays keep an ordered render-side tombstone keyed on the same id.
pub fn is_session_ended(session_id: &str) -> bool {
    ended_sessions().lock().unwrap().contains(session_id)
}

/// Run an ordered lifecycle operation only while `session_id` is live.
///
/// The ended-session lock stays held through `operation`, so a concurrent
/// [`fire_session_end`] cannot mark the session and enqueue its cleanup between
/// the live check and the operation. This is intended for short, non-blocking
/// lifecycle queue writes such as cursor revival.
pub fn with_live_session<R>(session_id: &str, operation: impl FnOnce() -> R) -> Option<R> {
    if !is_trackable(session_id) {
        return Some(operation());
    }
    let ended = ended_sessions().lock().unwrap();
    if ended.contains(session_id) {
        return None;
    }
    Some(operation())
}

/// Revive a previously-ended session id by clearing its tombstone, so a fresh
/// `start_session` with a recycled id works as a caller would expect: the id
/// becomes live again and its actions stop being rejected by the resurrection
/// guard. Returns whether the id had actually been ended (i.e. was revived).
///
/// This is the deliberate, EXPLICIT counterpart to the resurrection guard. The
/// guard exists so a *stray late action* on a dead id can't silently re-create
/// session-owned state; reviving requires an explicit `start_session` re-declare
/// of the same id, which is exactly what a caller reusing an id intends. No-op
/// for the anonymous fallback (`"default"` / empty), which is never tracked.
pub fn revive_session(session_id: &str) -> bool {
    if !is_trackable(session_id) {
        return false;
    }
    // Serialize revivals through the hook lock, but do not hold the ended-set
    // lock while invoking callbacks. The tombstone remains present while hooks
    // enqueue their ordered lifecycle events, so concurrent actions still see
    // the session as ended. Reentrant hooks may safely query that state.
    let hooks = revive_hooks().lock().unwrap();
    if !ended_sessions().lock().unwrap().contains(session_id) {
        return false;
    }
    for hook in hooks.iter() {
        hook(session_id);
    }
    ended_sessions().lock().unwrap().remove(session_id)
}

/// Record activity for an explicit session id, resetting its idle-TTL clock.
/// Called at the daemon boundary on every tool call that carries an explicit
/// `session`. No-op for the anonymous fallback (`"default"` / empty) and for a
/// session that has already ended (so a late in-flight call can't resurrect a
/// reaped session's TTL entry).
pub fn touch_session(session_id: &str) {
    if !is_trackable(session_id) || is_session_ended(session_id) {
        return;
    }
    activity()
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), Instant::now());
}

/// End a session explicitly (the `end_session` tool / `session end` CLI verb):
/// drop its idle-TTL entry and fan `fire_session_end` out to every cleanup hook
/// (overlay remove, recording stop, config-override clear). Idempotent via
/// `fire_session_end`'s dedupe. No-op for the anonymous fallback.
pub fn end_session(session_id: &str) {
    if !is_trackable(session_id) {
        return;
    }
    activity().lock().unwrap().remove(session_id);
    fire_session_end(session_id);
}

/// End every session whose last activity is older than `ttl`, returning the ids
/// ended. This is the idle-TTL sweep the daemon runs periodically: a
/// caller-declared session is no longer tied to a connection's lifetime, so a
/// run that finishes (or crashes) without calling `end_session` is reclaimed
/// here instead of leaking its cursor / recording. Sessions touched within the
/// TTL are left untouched.
pub fn evict_idle(ttl: Duration) -> Vec<String> {
    let now = Instant::now();
    let stale: Vec<String> = {
        let map = activity().lock().unwrap();
        map.iter()
            .filter(|(_, last)| now.duration_since(**last) >= ttl)
            .map(|(id, _)| id.clone())
            .collect()
    };
    for id in &stale {
        end_session(id);
    }
    stale
}

/// Number of sessions with a live idle-TTL entry. Diagnostics only.
pub fn active_session_count() -> usize {
    activity().lock().unwrap().len()
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

    #[test]
    fn touch_then_evict_by_ttl() {
        let sid = "test-ttl-session-DDEEFF";
        touch_session(sid);
        // A huge TTL leaves it alone (just touched).
        assert!(evict_idle(Duration::from_secs(3600))
            .iter()
            .all(|s| s != sid));
        // A zero TTL treats any prior activity as idle → evicts it.
        let evicted = evict_idle(Duration::ZERO);
        assert!(
            evicted.iter().any(|s| s == sid),
            "zero-TTL must evict a touched session"
        );
        assert!(is_session_ended(sid), "evicted session is ended");
    }

    #[test]
    fn anonymous_ids_are_never_tracked() {
        touch_session("default");
        touch_session("");
        // Neither shows up under a zero-TTL sweep (they were never inserted).
        let evicted = evict_idle(Duration::ZERO);
        assert!(!evicted.iter().any(|s| s == "default" || s.is_empty()));
    }

    #[test]
    fn end_session_is_explicit_teardown() {
        let sid = "test-end-session-112233";
        touch_session(sid);
        end_session(sid);
        assert!(is_session_ended(sid));
        // Its TTL entry is gone, so a later sweep doesn't re-fire for it.
        assert!(!evict_idle(Duration::ZERO).iter().any(|s| s == sid));
    }

    #[test]
    fn revive_clears_the_tombstone_for_an_ended_id() {
        let sid = "test-revive-session-445566";
        touch_session(sid);
        end_session(sid);
        assert!(is_session_ended(sid), "ended id is tombstoned");

        // Explicit re-declare revives it: tombstone cleared, returns true.
        assert!(revive_session(sid), "revive reports the id was ended");
        assert!(!is_session_ended(sid), "revived id is live again");

        // Reviving a live (or never-ended) id is a no-op returning false.
        assert!(!revive_session(sid), "reviving a live id is a no-op");
        assert!(!revive_session("test-never-ended-778899"));
    }

    #[test]
    fn revive_notifies_hooks_once_after_an_actual_end() {
        let sid = "test-revive-hook-session-A1B2C3";
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_hook = calls.clone();
        let expected = sid.to_owned();
        register_session_revive_hook(move |got| {
            if got == expected {
                calls_for_hook.fetch_add(1, Ordering::Relaxed);
            }
        });

        end_session(sid);
        assert!(revive_session(sid));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!revive_session(sid));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn revive_hook_can_reenter_session_state_while_tombstone_is_still_present() {
        let sid = "test-revive-reentrant-session-D4E5F6";
        let saw_ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let expected = sid.to_owned();
        let saw_ended_for_hook = saw_ended.clone();
        register_session_revive_hook(move |got| {
            if got == expected {
                saw_ended_for_hook.store(is_session_ended(got), Ordering::Relaxed);
            }
        });
        end_session(sid);
        assert!(revive_session(sid));
        assert!(saw_ended.load(Ordering::Relaxed));
        assert!(!is_session_ended(sid));
    }

    #[test]
    fn concurrent_double_revive_enqueues_hooks_once() {
        let sid = "test-double-revive-session-E5F6A7";
        let calls = Arc::new(AtomicUsize::new(0));
        let entered_hook = Arc::new(std::sync::Barrier::new(2));
        let release_hook = Arc::new(std::sync::Barrier::new(2));
        let expected = sid.to_owned();
        let calls_for_hook = calls.clone();
        let entered_for_hook = entered_hook.clone();
        let release_for_hook = release_hook.clone();
        register_session_revive_hook(move |got| {
            if got == expected {
                calls_for_hook.fetch_add(1, Ordering::Relaxed);
                entered_for_hook.wait();
                release_for_hook.wait();
            }
        });
        end_session(sid);

        let first_sid = sid.to_owned();
        let first = std::thread::spawn(move || revive_session(&first_sid));
        entered_hook.wait();

        let second_sid = sid.to_owned();
        let second = std::thread::spawn(move || revive_session(&second_sid));

        release_hook.wait();
        assert!(first.join().unwrap());
        assert!(!second.join().unwrap());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn revive_is_noop_for_anonymous_ids() {
        // The anonymous fallback is never tracked, so there is nothing to revive.
        assert!(!revive_session("default"));
        assert!(!revive_session(""));
    }

    #[test]
    fn live_session_operation_orders_before_concurrent_end_cleanup() {
        use std::sync::{Arc, Barrier};

        let sid = "test-live-operation-order-1A2B3C";
        let events = Arc::new(Mutex::new(Vec::new()));
        let hook_events = events.clone();
        register_session_end_hook(move |ended| {
            if ended == sid {
                hook_events.lock().unwrap().push("remove");
            }
        });

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_events = events.clone();
        let worker_entered = entered.clone();
        let worker_release = release.clone();
        let worker = std::thread::spawn(move || {
            with_live_session(sid, || {
                worker_events.lock().unwrap().push("revive");
                worker_entered.wait();
                worker_release.wait();
            })
        });

        entered.wait();
        let ender = std::thread::spawn(move || fire_session_end(sid));
        release.wait();
        assert!(worker.join().unwrap().is_some());
        ender.join().unwrap();
        assert_eq!(&*events.lock().unwrap(), &["revive", "remove"]);
    }
}
