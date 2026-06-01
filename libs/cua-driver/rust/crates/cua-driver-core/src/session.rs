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

type SessionEndHook = Box<dyn Fn(&str) + Send + Sync>;
type SessionNameHook = Box<dyn Fn(&str, &str) + Send + Sync>;

static SESSION_END_HOOKS: OnceLock<Mutex<Vec<SessionEndHook>>> = OnceLock::new();

/// Hooks invoked with `(session_id, name)` whenever a session's name is set
/// for the FIRST time (write-once). Mirrors `SESSION_END_HOOKS`: each platform
/// registers a `Fn(&str, &str)` once at startup so it can push the friendly
/// label to its overlay synchronously (macOS), with no reverse core→platform
/// dependency. A no-op on platforms that register nothing.
static SESSION_NAME_HOOKS: OnceLock<Mutex<Vec<SessionNameHook>>> = OnceLock::new();

/// Write-once friendly NAME for a session, keyed by `session_id`. First set
/// wins; later sets are no-ops (immutable for the session lifetime). Sanitized
/// on set. Cleared on `fire_session_end`. Read by the overlay (`session_label`)
/// so each cursor carries a human-readable label. Mirrors `ENDED_SESSIONS`'
/// process-global, unbounded-but-tiny shape.
static SESSION_NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

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

fn name_hooks() -> &'static Mutex<Vec<SessionNameHook>> {
    SESSION_NAME_HOOKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn session_names() -> &'static Mutex<HashMap<String, String>> {
    SESSION_NAMES.get_or_init(|| Mutex::new(HashMap::new()))
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
    // Clear the write-once name store. The name registry is cross-platform core
    // state (Linux/Windows read it too), so cleanup belongs here, not only in
    // the macOS session-end hook. Guard "default" so the anonymous cursor's
    // name survives, matching the cursor path's defensive stance. Take the
    // SESSION_NAMES lock independently — never nested under ended_sessions() or
    // a hook lock — to keep the no-deadlock invariant.
    if session_id != "default" {
        session_names().lock().unwrap().remove(session_id);
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

// ── Write-once per-session name ────────────────────────────────────────────────

/// Sanitize a raw name into a single-line, control-char-free, length-capped
/// debug label. Pure + unit-tested. Trims, drops every control char (so `\n`,
/// `\r`, `\t` and all control codes collapse to a single line), caps to 24
/// chars (char-count, UTF-8 safe), then trims trailing whitespace so a mid-space
/// truncation looks clean.
fn sanitize_session_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(24)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Register a callback invoked with `(session_id, effective_name)` the FIRST
/// time a session is named (write-once). macOS registers this to push the
/// label to its overlay synchronously so a `name_session` shows on the next
/// frame. No-op on platforms that register nothing.
pub fn register_session_name_hook(hook: impl Fn(&str, &str) + Send + Sync + 'static) {
    name_hooks().lock().unwrap().push(Box::new(hook));
}

/// Set the friendly name for a session, write-once and immutable. The FIRST
/// non-empty set wins; later sets are no-ops that return the existing value
/// unchanged (idempotent). Returns the effective (sanitized) name.
///
/// - On an already-ended session: no-op, returns the current name (or the
///   sanitized candidate as a courtesy), mirroring the platform config
///   registry's ended-guard.
/// - On an empty sanitized candidate: does NOT insert, so a garbage/whitespace
///   first call can't permanently consume the write-once slot and blank the
///   session — a later valid name still wins.
pub fn set_session_name(session_id: &str, raw: &str) -> String {
    let s = sanitize_session_name(raw);
    if is_session_ended(session_id) {
        return session_name(session_id).unwrap_or(s);
    }
    if s.is_empty() {
        return session_name(session_id).unwrap_or_default();
    }
    // First set wins; re-calls return the EXISTING value unchanged.
    let (effective, is_first) = {
        let mut map = session_names().lock().unwrap();
        let before_len = map.len();
        let val = map.entry(session_id.to_owned()).or_insert_with(|| s.clone()).clone();
        (val.clone(), map.len() != before_len)
    };
    // Fan the first-write out to the name hooks (overlay rename). Never hold the
    // SESSION_NAMES lock across the hook call.
    if is_first {
        for hook in name_hooks().lock().unwrap().iter() {
            hook(session_id, &effective);
        }
    }
    effective
}

/// The friendly name set for a session, or `None` if unnamed.
pub fn session_name(session_id: &str) -> Option<String> {
    session_names().lock().unwrap().get(session_id).cloned()
}

/// A short, always-readable auto tag derived from the `session_id`. For the
/// proxy mint shape `"mcp-<pid>-<nanos>"` this returns `"mcp-<pid>"`; ids with
/// fewer than two `-`-separated segments are returned verbatim (so an explicit
/// `cursor_id` degrades gracefully), and `"default"` / `""` pass through.
pub fn session_short_tag(session_id: &str) -> String {
    if session_id == "default" || session_id.is_empty() {
        return session_id.to_owned();
    }
    let mut parts = session_id.split('-');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}-{b}"),
        _ => session_id.to_owned(),
    }
}

/// The label to show beside a cursor: the friendly name if set, else the short
/// auto tag. Single source of the name-vs-tag fallback policy; the overlay
/// calls this so there is ALWAYS a readable label.
pub fn session_label(session_id: &str) -> String {
    session_name(session_id).unwrap_or_else(|| session_short_tag(session_id))
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
    fn set_session_name_is_write_once() {
        let sid = "test-name-writeonce-A1";
        assert_eq!(set_session_name(sid, "First"), "First");
        // A second set with a DIFFERENT name must be a no-op returning the first.
        assert_eq!(set_session_name(sid, "Second"), "First");
        assert_eq!(session_name(sid).as_deref(), Some("First"));
    }

    #[test]
    fn empty_first_call_does_not_consume_slot() {
        let sid = "test-name-empty-A2";
        // Whitespace-only first call sanitizes to "" and must NOT lock the slot.
        assert_eq!(set_session_name(sid, "   "), "");
        assert_eq!(session_name(sid), None);
        // A later valid name still wins.
        assert_eq!(set_session_name(sid, "Real"), "Real");
        assert_eq!(session_name(sid).as_deref(), Some("Real"));
    }

    #[test]
    fn sanitize_caps_and_strips() {
        // Control chars (newline/tab/CR) dropped → single line.
        assert_eq!(sanitize_session_name("a\nb\tc\rd"), "abcd");
        // Trim leading/trailing whitespace.
        assert_eq!(sanitize_session_name("  hi  "), "hi");
        // Cap at 24 chars (char count).
        let long = "x".repeat(40);
        assert_eq!(sanitize_session_name(&long).chars().count(), 24);
        // Mid-space truncation trims trailing whitespace.
        let s = sanitize_session_name("aaaaaaaaaaaaaaaaaaaaaaa b extra");
        assert!(!s.ends_with(' '), "trailing space must be trimmed: {s:?}");
    }

    #[test]
    fn short_tag_derivation() {
        assert_eq!(session_short_tag("mcp-51088-1700000000"), "mcp-51088");
        assert_eq!(session_short_tag("default"), "default");
        assert_eq!(session_short_tag(""), "");
        assert_eq!(session_short_tag("singlesegment"), "singlesegment");
    }

    #[test]
    fn label_prefers_name_then_tag() {
        let sid = "test-label-mcp-9999-123";
        // Unnamed → short tag.
        assert_eq!(session_label(sid), "test-label");
        // Named → the name.
        set_session_name(sid, "MyTask");
        assert_eq!(session_label(sid), "MyTask");
    }

    #[test]
    fn session_end_clears_name_but_guards_default() {
        let sid = "test-name-clear-mcp-7-1";
        set_session_name(sid, "Doomed");
        assert_eq!(session_name(sid).as_deref(), Some("Doomed"));
        fire_session_end(sid);
        assert_eq!(session_name(sid), None, "name must be cleared on session end");

        // "default" name must survive a session_end for "default".
        set_session_name("default", "Anon");
        fire_session_end("default");
        assert_eq!(session_name("default").as_deref(), Some("Anon"));
    }

    #[test]
    fn set_on_ended_session_is_noop() {
        let sid = "test-name-ended-B3";
        fire_session_end(sid);
        // Naming a dead session is a no-op (it was never named).
        let got = set_session_name(sid, "TooLate");
        assert_eq!(got, "TooLate", "courtesy return of the candidate");
        assert_eq!(session_name(sid), None, "dead session must stay unnamed");
    }
}
