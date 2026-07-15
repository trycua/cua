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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type SessionEndHook = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDeclaration {
    StartSession,
    ImplicitFirstAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEndReason {
    Explicit,
    IdleTimeout,
    ProcessExit,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTransport {
    Cli,
    Daemon,
    McpStdio,
    McpHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStartObservation {
    pub declaration: SessionDeclaration,
    pub revived: bool,
    pub transport: SessionTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyleCategory {
    Default,
    BuiltinArrow,
    BuiltinTeardrop,
    CustomIcon,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorColorSource {
    AutomaticPalette,
    Custom,
    Unknown,
}

/// Platform-neutral, content-free cursor state captured immediately before a
/// session's platform cleanup hooks remove the underlying cursor entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorOutcomeObservation {
    pub observed: bool,
    pub enabled: bool,
    pub style: CursorStyleCategory,
    pub color_source: CursorColorSource,
    pub label_set: bool,
    pub motion_customized: bool,
    pub active_cursor_count: usize,
}

/// Convert platform cursor fields to fixed categories without retaining any
/// raw icon, color, label, identifier, or motion value.
pub fn bounded_cursor_outcome(
    observed: bool,
    enabled: bool,
    icon: Option<&str>,
    color: Option<&str>,
    label: Option<&str>,
    motion_customized: bool,
    active_cursor_count: usize,
) -> CursorOutcomeObservation {
    let style = if !observed {
        CursorStyleCategory::Unknown
    } else {
        match icon.map(str::trim).filter(|value| !value.is_empty()) {
            None => CursorStyleCategory::Default,
            Some(value) if value.eq_ignore_ascii_case("arrow") => {
                CursorStyleCategory::BuiltinArrow
            }
            Some(value) if value.eq_ignore_ascii_case("teardrop") => {
                CursorStyleCategory::BuiltinTeardrop
            }
            Some(_) => CursorStyleCategory::CustomIcon,
        }
    };
    let color_source = if !observed {
        CursorColorSource::Unknown
    } else {
        match color.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("#00FFFF") | Some("#00ffff") => CursorColorSource::AutomaticPalette,
            Some(_) => CursorColorSource::Custom,
        }
    };
    CursorOutcomeObservation {
        observed,
        enabled: observed && enabled,
        style,
        color_source,
        label_set: observed && label.is_some_and(|value| !value.trim().is_empty()),
        motion_customized: observed && motion_customized,
        active_cursor_count,
    }
}

/// Process-local sink for bounded session telemetry.
///
/// `session_id` is supplied only so an observer can update private in-memory
/// state. Implementations must never serialize, hash, log, or otherwise export
/// it. The observation structs and completion outcome contain the complete
/// allowlisted telemetry boundary.
pub trait SessionObserver: Send + Sync + 'static {
    fn on_session_started(&self, session_id: &str, observation: SessionStartObservation);
    fn on_tool_completed(
        &self,
        session_id: &str,
        transport: SessionTransport,
        computer_action: bool,
        outcome: &crate::server::ToolCompletionObservation,
    );
    fn on_session_ended(
        &self,
        session_id: &str,
        reason: SessionEndReason,
        cursor: Option<CursorOutcomeObservation>,
    );
}

static SESSION_OBSERVER: OnceLock<Arc<dyn SessionObserver>> = OnceLock::new();
type CursorOutcomeReader = Arc<dyn Fn(&str) -> CursorOutcomeObservation + Send + Sync>;
static CURSOR_OUTCOME_READER: OnceLock<Mutex<Option<CursorOutcomeReader>>> = OnceLock::new();

pub fn set_session_observer(observer: Arc<dyn SessionObserver>) -> bool {
    SESSION_OBSERVER.set(observer).is_ok()
}

/// Register the platform's bounded cursor-state reader. The raw session key is
/// used only for the synchronous process-local lookup; the callback returns a
/// struct that cannot contain raw cursor values.
pub fn set_cursor_outcome_reader(reader: CursorOutcomeReader) -> bool {
    *CURSOR_OUTCOME_READER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(reader);
    true
}

/// Private per-call context. The raw caller session id never crosses into a
/// serialized observation; it is retained only until the bounded completion
/// callback updates the process-local aggregate.
pub struct SessionToolContext {
    session_id: String,
    transport: SessionTransport,
    computer_action: bool,
}

impl SessionToolContext {
    pub fn complete(self, outcome: &crate::server::ToolCompletionObservation) {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_tool_completed(
                &self.session_id,
                self.transport,
                self.computer_action,
                outcome,
            );
        }
    }
}

static SESSION_END_HOOKS: OnceLock<Mutex<Vec<SessionEndHook>>> = OnceLock::new();

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

fn is_computer_action(tool_name: &str, args: &serde_json::Value) -> bool {
    if tool_name == "page" {
        return matches!(
            args.get("action").and_then(serde_json::Value::as_str),
            Some(
                "click_element"
                    | "insert_text"
                    | "type_keystrokes"
                    | "enable_javascript_apple_events"
            )
        );
    }
    crate::tool::default_capabilities_for(tool_name)
        .iter()
        .any(|capability| {
            capability.starts_with("input.pointer.")
                || capability.starts_with("input.keyboard.")
                || matches!(
                    capability.as_str(),
                    "app.launch" | "app.kill" | "window.activate"
                )
        })
}

/// Begin bounded observation for a known tool call carrying a public,
/// caller-declared `session`. Reserved `_session_id` fallbacks and anonymous
/// identities are deliberately ignored.
pub fn begin_tool_call(
    tool_name: &str,
    args: &serde_json::Value,
    known_tool: bool,
    transport: SessionTransport,
) -> Option<SessionToolContext> {
    if !known_tool {
        return None;
    }
    let session_id = args
        .get("session")
        .and_then(serde_json::Value::as_str)
        .filter(|id| is_trackable(id))?;
    let is_start = tool_name == "start_session";
    let is_end = tool_name == "end_session";
    let revived = is_start && is_session_ended(session_id);
    if is_session_ended(session_id) && !is_start {
        return None;
    }

    touch_session(session_id);
    if !is_end {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_session_started(
                session_id,
                SessionStartObservation {
                    declaration: if is_start {
                        SessionDeclaration::StartSession
                    } else {
                        SessionDeclaration::ImplicitFirstAction
                    },
                    revived,
                    transport,
                },
            );
        }
    }

    SESSION_OBSERVER.get().map(|_| SessionToolContext {
        session_id: session_id.to_owned(),
        transport,
        computer_action: is_computer_action(tool_name, args),
    })
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
/// recording-stop run exactly once. Returns `true` only for that first fire;
/// later calls return `false`. The first fire still returns `true` when no
/// hooks are registered.
pub fn fire_session_end(session_id: &str) -> bool {
    // Mark-then-fan-out under a short critical section, releasing the lock
    // before running hooks (hooks may be slow / re-entrant and must not hold
    // the dedupe lock).
    {
        let mut ended = ended_sessions().lock().unwrap();
        if !ended.insert(session_id.to_owned()) {
            return false; // already ended — idempotent no-op.
        }
    }
    for hook in hooks().lock().unwrap().iter() {
        hook(session_id);
    }
    true
}

/// Whether `fire_session_end` has already run for this `session_id`. The
/// daemon-side authority for "this session is permanently gone"; the macOS
/// overlay keeps its own render-side tombstone keyed on the same id.
pub fn is_session_ended(session_id: &str) -> bool {
    ended_sessions().lock().unwrap().contains(session_id)
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
    end_session_with_reason(session_id, SessionEndReason::Explicit);
}

fn end_session_with_reason(session_id: &str, reason: SessionEndReason) {
    if !is_trackable(session_id) {
        return;
    }
    activity().lock().unwrap().remove(session_id);
    let cursor_reader = CURSOR_OUTCOME_READER
        .get()
        .and_then(|reader| reader.lock().unwrap().clone());
    let cursor = cursor_reader.map(|reader| reader(session_id));
    if fire_session_end(session_id) {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_session_ended(session_id, reason, cursor);
        }
    }
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
        end_session_with_reason(id, SessionEndReason::IdleTimeout);
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

    #[derive(Default)]
    struct ProbeObserver {
        starts: Mutex<Vec<(String, SessionStartObservation)>>,
        ends: Mutex<Vec<(String, SessionEndReason, Option<CursorOutcomeObservation>)>>,
    }

    impl SessionObserver for ProbeObserver {
        fn on_session_started(&self, id: &str, observation: SessionStartObservation) {
            self.starts.lock().unwrap().push((id.to_owned(), observation));
        }
        fn on_tool_completed(
            &self,
            _: &str,
            _: SessionTransport,
            _: bool,
            _: &crate::server::ToolCompletionObservation,
        ) {
        }
        fn on_session_ended(
            &self,
            id: &str,
            reason: SessionEndReason,
            cursor: Option<CursorOutcomeObservation>,
        ) {
            self.ends
                .lock()
                .unwrap()
                .push((id.to_owned(), reason, cursor));
        }
    }

    fn probe_observer() -> Arc<ProbeObserver> {
        static PROBE: OnceLock<Arc<ProbeObserver>> = OnceLock::new();
        PROBE
            .get_or_init(|| {
                let probe = Arc::new(ProbeObserver::default());
                let _ = set_session_observer(probe.clone());
                probe
            })
            .clone()
    }

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
        assert!(evict_idle(Duration::from_secs(3600)).iter().all(|s| s != sid));
        // A zero TTL treats any prior activity as idle → evicts it.
        let evicted = evict_idle(Duration::ZERO);
        assert!(evicted.iter().any(|s| s == sid), "zero-TTL must evict a touched session");
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
    fn cursor_outcomes_are_fixed_categories_without_raw_values() {
        let custom = bounded_cursor_outcome(
            true,
            true,
            Some("/private/customer/cursor.svg"),
            Some("private-brand-color"),
            Some("private agent label"),
            true,
            7,
        );
        assert_eq!(custom.style, CursorStyleCategory::CustomIcon);
        assert_eq!(custom.color_source, CursorColorSource::Custom);
        assert!(custom.label_set);
        assert!(custom.motion_customized);
        assert_eq!(custom.active_cursor_count, 7);
        let debug = format!("{custom:?}");
        for forbidden in ["/private/customer", "private-brand", "private agent"] {
            assert!(!debug.contains(forbidden), "cursor outcome leaked {forbidden}: {debug}");
        }

        let unknown = bounded_cursor_outcome(
            false,
            true,
            Some("arrow"),
            Some("#00FFFF"),
            Some("label"),
            true,
            0,
        );
        assert_eq!(unknown.style, CursorStyleCategory::Unknown);
        assert_eq!(unknown.color_source, CursorColorSource::Unknown);
        assert!(!unknown.enabled);
        assert!(!unknown.label_set);
        assert!(!unknown.motion_customized);
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
    fn revive_is_noop_for_anonymous_ids() {
        // The anonymous fallback is never tracked, so there is nothing to revive.
        assert!(!revive_session("default"));
        assert!(!revive_session(""));
    }

    #[test]
    fn tool_context_requires_a_public_session_and_uses_fixed_action_classes() {
        let _ = probe_observer();
        assert!(begin_tool_call(
            "click",
            &serde_json::json!({"_session_id": "private-fallback"}),
            true,
            SessionTransport::McpStdio,
        )
        .is_none());
        assert!(begin_tool_call(
            "click",
            &serde_json::json!({"session": "default"}),
            true,
            SessionTransport::McpStdio,
        )
        .is_none());

        let pointer = begin_tool_call(
            "click",
            &serde_json::json!({"session": "test-action-pointer-AB12"}),
            true,
            SessionTransport::McpStdio,
        )
        .unwrap();
        assert!(pointer.computer_action);

        let page_write = begin_tool_call(
            "page",
            &serde_json::json!({
                "session": "test-action-page-write-CD34",
                "action": "insert_text",
                "text": "not retained"
            }),
            true,
            SessionTransport::McpHttp,
        )
        .unwrap();
        assert!(page_write.computer_action);
        assert!(!format!("{}", page_write.computer_action).contains("not retained"));

        let page_read = begin_tool_call(
            "page",
            &serde_json::json!({
                "session": "test-action-page-read-EF56",
                "action": "query_dom",
                "selector": "private selector"
            }),
            true,
            SessionTransport::McpHttp,
        )
        .unwrap();
        assert!(!page_read.computer_action);

        let state_read = begin_tool_call(
            "get_window_state",
            &serde_json::json!({"session": "test-action-read-GH78"}),
            true,
            SessionTransport::Daemon,
        )
        .unwrap();
        assert!(!state_read.computer_action);
    }

    #[test]
    fn observer_distinguishes_explicit_idle_revival_and_control_cleanup() {
        let probe = probe_observer();
        let _ = set_cursor_outcome_reader(Arc::new(|_| {
            bounded_cursor_outcome(true, true, Some("arrow"), None, None, false, 2)
        }));
        let explicit = "test-observer-explicit-IJ90";
        begin_tool_call(
            "start_session",
            &serde_json::json!({"session": explicit}),
            true,
            SessionTransport::McpStdio,
        )
        .unwrap();
        end_session(explicit);

        let idle = "test-observer-idle-KL12";
        begin_tool_call(
            "click",
            &serde_json::json!({"session": idle}),
            true,
            SessionTransport::McpHttp,
        )
        .unwrap();
        end_session_with_reason(idle, SessionEndReason::IdleTimeout);

        let revived = "test-observer-revived-MN34";
        touch_session(revived);
        end_session(revived);
        begin_tool_call(
            "start_session",
            &serde_json::json!({"session": revived}),
            true,
            SessionTransport::Daemon,
        )
        .unwrap();

        let control = "test-observer-control-OP56";
        begin_tool_call(
            "click",
            &serde_json::json!({"session": control}),
            true,
            SessionTransport::McpStdio,
        )
        .unwrap();
        fire_session_end(control);

        let starts = probe.starts.lock().unwrap();
        assert!(starts.iter().any(|(id, observation)| {
            id == explicit
                && observation.declaration == SessionDeclaration::StartSession
                && !observation.revived
        }));
        assert!(starts.iter().any(|(id, observation)| {
            id == idle
                && observation.declaration == SessionDeclaration::ImplicitFirstAction
                && observation.transport == SessionTransport::McpHttp
        }));
        assert!(starts.iter().any(|(id, observation)| id == revived && observation.revived));
        drop(starts);

        let ends = probe.ends.lock().unwrap();
        assert!(ends.iter().any(|(id, reason, cursor)| {
            id == explicit && *reason == SessionEndReason::Explicit
                && cursor.is_some_and(|value| {
                    value.style == CursorStyleCategory::BuiltinArrow
                        && value.active_cursor_count == 2
                })
        }));
        assert!(ends.iter().any(|(id, reason, _)| {
            id == idle && *reason == SessionEndReason::IdleTimeout
        }));
        assert!(!ends.iter().any(|(id, _, _)| id == control));
    }
}
