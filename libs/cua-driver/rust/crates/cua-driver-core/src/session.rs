//! Session lifecycle hooks.
//!
//! The public SDK runtime owns one shared platform registry; the cua-driver
//! daemon and every `cua-driver mcp` proxy consume that runtime downstream. A
//! proxy-minted `session_id` (carried in the daemon request envelope) lets the
//! daemon OWN and CLEAN UP per-session state.
//!
//! Recording ownership lives on the core `RecordingSession` directly. But some
//! session-scoped state is platform-specific (e.g. macOS per-session config
//! overrides in `platform-macos::tools::SessionConfigRegistry`) and the daemon
//! cannot reach into a platform crate's private `ToolState`. This module
//! bridges that gap with a small process-global list
//! of cleanup callbacks: each platform registers a `Fn(&str)` once at startup,
//! and the daemon's `session_end` arm fans the disconnecting `session_id` out
//! to all of them.
//!
//! This mirrors the existing screenshot/AX-snapshot callback pattern in
//! `recording.rs` — a registry-free, platform-pluggable hook set with no
//! reverse coupling from core into the platform crates.

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use cua_driver_contract::{CaptureScope, EscalationReason};

type SessionEndHook = Arc<dyn Fn(&str) + Send + Sync>;

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

/// Closed entry-surface category for one explicit session episode. This is
/// deliberately independent from transport: Python and TypeScript SDKs both
/// use the direct daemon transport, while MCP can use stdio or HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionClientKind {
    Cli,
    Direct,
    Mcp,
    PythonSdk,
    TypescriptSdk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStartObservation {
    pub declaration: SessionDeclaration,
    pub revived: bool,
    pub transport: SessionTransport,
    pub client_kind: SessionClientKind,
    pub capture_scope: CaptureScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionObservationState {
    pub ended: bool,
    pub capture_scope: CaptureScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorThemeCategory {
    Default,
    Custom,
    Unknown,
}

/// Platform-neutral, content-free cursor state captured immediately before a
/// session's platform cleanup hooks remove the underlying cursor entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorOutcomeObservation {
    pub observed: bool,
    pub enabled: bool,
    pub theme: CursorThemeCategory,
    pub motion_customized: bool,
    pub active_cursor_count: usize,
}

/// Convert platform cursor fields to fixed categories without retaining any
/// raw theme identifier or motion value.
pub fn bounded_cursor_outcome(
    observed: bool,
    enabled: bool,
    theme_id: Option<&str>,
    motion_customized: bool,
    active_cursor_count: usize,
) -> CursorOutcomeObservation {
    let theme = if !observed {
        CursorThemeCategory::Unknown
    } else {
        match theme_id.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("cua.default") => CursorThemeCategory::Default,
            Some(_) => CursorThemeCategory::Custom,
        }
    };
    CursorOutcomeObservation {
        observed,
        enabled: observed && enabled,
        theme,
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
        escalation_reason: Option<EscalationReason>,
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
static CURSOR_OUTCOME_READERS: OnceLock<Mutex<HashMap<u64, CursorOutcomeReader>>> = OnceLock::new();
static NEXT_CURSOR_OUTCOME_READER_ID: AtomicU64 = AtomicU64::new(1);

pub fn set_session_observer(observer: Arc<dyn SessionObserver>) -> bool {
    SESSION_OBSERVER.set(observer).is_ok()
}

/// Register the platform's bounded cursor-state reader. The raw session key is
/// used only for the synchronous process-local lookup; the callback returns a
/// struct that cannot contain raw cursor values.
pub fn set_cursor_outcome_reader(reader: CursorOutcomeReader) -> bool {
    CURSOR_OUTCOME_READERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(0, reader);
    true
}

pub fn register_scoped_cursor_outcome_reader(
    reader: CursorOutcomeReader,
) -> CursorOutcomeReaderRegistration {
    let id = NEXT_CURSOR_OUTCOME_READER_ID.fetch_add(1, Ordering::Relaxed);
    CURSOR_OUTCOME_READERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(id, reader);
    CursorOutcomeReaderRegistration { id }
}

pub struct CursorOutcomeReaderRegistration {
    id: u64,
}

impl Drop for CursorOutcomeReaderRegistration {
    fn drop(&mut self) {
        if let Some(readers) = CURSOR_OUTCOME_READERS.get() {
            readers.lock().unwrap().remove(&self.id);
        }
    }
}

/// Private per-call context. The raw caller session id never crosses into a
/// serialized observation; it is retained only until the bounded completion
/// callback updates the process-local aggregate.
pub struct SessionToolContext {
    session_id: String,
    transport: SessionTransport,
    computer_action: bool,
    escalation_reason: Option<EscalationReason>,
}

impl SessionToolContext {
    pub fn complete(self, outcome: &crate::server::ToolCompletionObservation) {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_tool_completed(
                &self.session_id,
                self.transport,
                self.computer_action,
                self.escalation_reason,
                outcome,
            );
        }
    }
}

static SESSION_END_HOOKS: OnceLock<Mutex<HashMap<u64, SessionEndHook>>> = OnceLock::new();
static NEXT_SESSION_END_HOOK_ID: AtomicU64 = AtomicU64::new(1);

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

/// Begin bounded observation for a known tool call carrying a public,
/// caller-declared `session`. Reserved `_session_id` fallbacks and anonymous
/// identities are deliberately ignored.
pub fn begin_tool_call(
    tool_name: &str,
    args: &serde_json::Value,
    known_tool: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
) -> Option<SessionToolContext> {
    begin_tool_call_with_state(tool_name, args, known_tool, transport, client_kind, None)
}

/// Begin observation with a runtime-owner-provided view of public session
/// state. Runtime owners use this seam because their mutable scope and
/// tombstone state is keyed by a private runtime generation, while telemetry
/// must continue to report the caller's stable public session label.
pub fn begin_tool_call_with_state(
    tool_name: &str,
    args: &serde_json::Value,
    known_tool: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    observed_state: Option<SessionObservationState>,
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
    let ended = observed_state
        .map(|state| state.ended)
        .unwrap_or_else(|| is_session_ended(session_id));
    let revived = is_start && ended;
    if ended && !is_start {
        return None;
    }

    let capture_scope = if is_start {
        args.get("capture_scope")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    } else {
        observed_state
            .map(|state| state.capture_scope)
            .or_else(|| crate::capture_scope::get_session(session_id).map(|state| state.policy))
            .unwrap_or_default()
    };
    let escalation_reason = (tool_name == "escalate_session")
        .then(|| args.get("reason").cloned())
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok());
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
                    client_kind,
                    capture_scope,
                },
            );
        }
    }

    SESSION_OBSERVER.get().map(|_| SessionToolContext {
        session_id: session_id.to_owned(),
        transport,
        computer_action: crate::server::is_computer_action(
            tool_name,
            crate::server::tool_operation(tool_name, Some(args)),
        ),
        escalation_reason,
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
/// Runtime generations that have received terminal revoke-all.
///
/// This latch is intentionally independent of grants and public session
/// labels. Once set, every later dispatch owned by the same runtime
/// generation fails closed until that runtime is destroyed.
static SUSPENDED_RUNTIME_SCOPES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn hooks() -> &'static Mutex<HashMap<u64, SessionEndHook>> {
    SESSION_END_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ended_sessions() -> &'static Mutex<HashSet<String>> {
    ENDED_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn suspended_runtime_scopes() -> &'static Mutex<HashSet<String>> {
    SUSPENDED_RUNTIME_SCOPES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Terminally suspend authorization for one private runtime generation.
///
/// The opaque scope is generated inside Cua and is never accepted from a
/// public tool argument.
pub fn suspend_runtime_scope(runtime_scope: &str) -> bool {
    suspended_runtime_scopes()
        .lock()
        .unwrap()
        .insert(runtime_scope.to_owned())
}

pub fn is_runtime_scope_suspended(runtime_scope: &str) -> bool {
    suspended_runtime_scopes()
        .lock()
        .unwrap()
        .contains(runtime_scope)
}

/// Forget a suspend latch only when its owning runtime is being destroyed.
pub fn forget_suspended_runtime_scope(runtime_scope: &str) -> bool {
    suspended_runtime_scopes()
        .lock()
        .unwrap()
        .remove(runtime_scope)
}

fn public_session_label(session_id: &str) -> &str {
    let Some(rest) = session_id.strip_prefix("__cua_runtime_") else {
        return session_id;
    };
    let Some((scope, public)) = rest.split_once(':') else {
        return session_id;
    };
    if scope.len() == 32 && scope.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        public
    } else {
        session_id
    }
}

/// Register a callback invoked with the disconnecting `session_id` whenever a
/// session ends (graceful proxy EOF → daemon `session_end`). Each platform
/// registers its session-scoped cleanup here once at startup. Idempotency and
/// "unknown session id" tolerance are the hook's responsibility — `session_end`
/// fires once per proxy exit, but a hook should treat a clear of an unseen id
/// as a no-op.
pub fn register_session_end_hook(hook: impl Fn(&str) + Send + Sync + 'static) {
    std::mem::forget(register_scoped_session_end_hook(hook));
}

/// Register a runtime-owned cleanup hook. Dropping the returned guard removes
/// the callback, preventing create/shutdown cycles from accumulating stale
/// platform and browser hooks.
pub fn register_scoped_session_end_hook(
    hook: impl Fn(&str) + Send + Sync + 'static,
) -> SessionEndHookRegistration {
    let id = NEXT_SESSION_END_HOOK_ID.fetch_add(1, Ordering::Relaxed);
    hooks().lock().unwrap().insert(id, Arc::new(hook));
    SessionEndHookRegistration { id }
}

pub struct SessionEndHookRegistration {
    id: u64,
}

impl Drop for SessionEndHookRegistration {
    fn drop(&mut self) {
        hooks().lock().unwrap().remove(&self.id);
    }
}

#[doc(hidden)]
pub fn session_end_hook_count() -> usize {
    hooks().lock().unwrap().len()
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
    crate::capture_scope::clear_session(session_id);
    let registered = hooks()
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for hook in registered {
        hook(session_id);
    }
    true
}

/// Revoke every currently tracked session. This is intentionally callable by
/// a local operator control path without requiring an authorization grant:
/// revocation can only remove authority, never create it.
pub fn revoke_all_sessions() -> usize {
    let sessions: Vec<String> = activity().lock().unwrap().keys().cloned().collect();
    for session in &sessions {
        end_session(session);
    }
    sessions.len()
}

/// Revoke only sessions owned by one runtime-private namespace.
pub fn revoke_sessions_with_prefix(prefix: &str) -> usize {
    let sessions: Vec<String> = activity()
        .lock()
        .unwrap()
        .keys()
        .filter(|session| session.starts_with(prefix))
        .cloned()
        .collect();
    for session in &sessions {
        end_session(session);
    }
    sessions.len()
}

/// Forget terminal tombstones owned by a runtime generation that is itself
/// being destroyed. Live runtimes must retain tombstones until an explicit
/// `start_session` revival; otherwise an operator revocation could be bypassed
/// by reusing the same public session label on another transport.
pub fn forget_ended_sessions_with_prefix(prefix: &str) -> usize {
    let mut ended = ended_sessions().lock().unwrap();
    let before = ended.len();
    ended.retain(|session| !session.starts_with(prefix));
    let forgotten = before - ended.len();
    drop(ended);
    crate::capture_scope::clear_sessions_with_prefix(prefix);
    forgotten
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

/// Record activity for an explicit runtime-private session id, resetting its
/// idle-TTL clock. Called at the authorized registry boundary after public
/// arguments have been mapped into their owning runtime namespace. No-op for
/// the anonymous fallback (`"default"` / empty) and for a session that has
/// already ended (so a late in-flight call can't resurrect a reaped session's
/// TTL entry).
pub fn touch_session(session_id: &str) {
    if !is_trackable(session_id) || is_session_ended(session_id) {
        return;
    }
    activity()
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), Instant::now());
}

#[doc(hidden)]
pub fn has_session_activity(session_id: &str) -> bool {
    activity().lock().unwrap().contains_key(session_id)
}

#[doc(hidden)]
pub fn session_idle_duration(session_id: &str) -> Option<Duration> {
    activity()
        .lock()
        .unwrap()
        .get(session_id)
        .map(Instant::elapsed)
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
    let mut cursor_readers = CURSOR_OUTCOME_READERS
        .get()
        .map(|readers| {
            readers
                .lock()
                .unwrap()
                .iter()
                .map(|(id, reader)| (*id, reader.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    cursor_readers.sort_by_key(|(id, _)| *id);
    let mut fallback = None;
    let cursor = cursor_readers.into_iter().find_map(|(_, reader)| {
        let outcome = reader(session_id);
        if outcome.observed {
            Some(outcome)
        } else {
            fallback.get_or_insert(outcome);
            None
        }
    });
    let cursor = cursor.or(fallback);
    if fire_session_end(session_id) {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_session_ended(public_session_label(session_id), reason, cursor);
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

/// Runtime-scoped form of [`evict_idle`]. The namespace prefix is minted by
/// the trusted runtime and never accepted from a tool argument.
pub fn evict_idle_with_prefix(ttl: Duration, prefix: &str) -> Vec<String> {
    let now = Instant::now();
    let stale: Vec<String> = {
        let map = activity().lock().unwrap();
        map.iter()
            .filter(|(id, last)| id.starts_with(prefix) && now.duration_since(**last) >= ttl)
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
            self.starts
                .lock()
                .unwrap()
                .push((id.to_owned(), observation));
        }
        fn on_tool_completed(
            &self,
            _: &str,
            _: SessionTransport,
            _: bool,
            _: Option<EscalationReason>,
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
    fn browser_mutations_are_computer_actions_but_reads_and_prepare_are_not() {
        let args = serde_json::json!({"session": "test-session"});
        for tool_name in [
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_set_input_files",
            "browser_download",
            "browser_pointer",
        ] {
            let operation = crate::server::tool_operation(tool_name, Some(&args));
            assert!(
                crate::server::is_computer_action(tool_name, operation),
                "tool={tool_name}"
            );
        }
        for tool_name in ["get_browser_state", "browser_prepare"] {
            let operation = crate::server::tool_operation(tool_name, Some(&args));
            assert!(
                !crate::server::is_computer_action(tool_name, operation),
                "tool={tool_name}"
            );
        }
        for action in ["accept", "dismiss"] {
            let args = serde_json::json!({"action": action});
            let operation = crate::server::tool_operation("browser_dialog", Some(&args));
            assert!(crate::server::is_computer_action(
                "browser_dialog",
                operation
            ));
        }
        let inspect = serde_json::json!({"action": "inspect"});
        let operation = crate::server::tool_operation("browser_dialog", Some(&inspect));
        assert!(!crate::server::is_computer_action(
            "browser_dialog",
            operation
        ));
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
    fn cursor_outcomes_are_fixed_categories_without_raw_values() {
        let custom = bounded_cursor_outcome(true, true, Some("private.customer.theme"), true, 7);
        assert_eq!(custom.theme, CursorThemeCategory::Custom);
        assert!(custom.motion_customized);
        assert_eq!(custom.active_cursor_count, 7);
        let debug = format!("{custom:?}");
        let forbidden = "private.customer.theme";
        assert!(
            !debug.contains(forbidden),
            "cursor outcome leaked {forbidden}: {debug}"
        );

        let unknown = bounded_cursor_outcome(false, true, Some("cua.default"), true, 0);
        assert_eq!(unknown.theme, CursorThemeCategory::Unknown);
        assert!(!unknown.enabled);
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
    fn runtime_revoke_retains_tombstones_until_revival_or_runtime_teardown() {
        let prefix = "__cua_runtime_00112233445566778899aabbccddeeff:";
        let sid = format!("{prefix}revoked-session");
        touch_session(&sid);

        assert_eq!(revoke_sessions_with_prefix(prefix), 1);
        assert!(is_session_ended(&sid));
        touch_session(&sid);
        assert!(
            !has_session_activity(&sid),
            "ordinary traffic must not resurrect a revoked runtime session"
        );

        assert!(revive_session(&sid));
        touch_session(&sid);
        assert!(has_session_activity(&sid));
        end_session(&sid);
        assert_eq!(forget_ended_sessions_with_prefix(prefix), 1);
        assert!(!is_session_ended(&sid));
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
            SessionClientKind::Mcp,
        )
        .is_none());
        assert!(begin_tool_call(
            "click",
            &serde_json::json!({"session": "default"}),
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .is_none());

        let pointer = begin_tool_call(
            "click",
            &serde_json::json!({"session": "test-action-pointer-AB12"}),
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
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
            SessionClientKind::Mcp,
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
            SessionClientKind::Mcp,
        )
        .unwrap();
        assert!(!page_read.computer_action);

        let state_read = begin_tool_call(
            "get_window_state",
            &serde_json::json!({"session": "test-action-read-GH78"}),
            true,
            SessionTransport::Daemon,
            SessionClientKind::Direct,
        )
        .unwrap();
        assert!(!state_read.computer_action);

        let escalation = begin_tool_call(
            "escalate_session",
            &serde_json::json!({
                "session": "test-action-escalate-IJ90",
                "reason": "no_window_target",
                "detail": "private free-form detail is not observed"
            }),
            true,
            SessionTransport::Daemon,
            SessionClientKind::TypescriptSdk,
        )
        .unwrap();
        assert_eq!(
            escalation.escalation_reason,
            Some(EscalationReason::NoWindowTarget)
        );
    }

    #[test]
    fn observer_distinguishes_explicit_idle_revival_and_control_cleanup() {
        let probe = probe_observer();
        let _ = set_cursor_outcome_reader(Arc::new(|_| {
            bounded_cursor_outcome(true, true, Some("cua.default"), false, 2)
        }));
        let explicit = "test-observer-explicit-IJ90";
        begin_tool_call(
            "start_session",
            &serde_json::json!({"session": explicit, "capture_scope": "desktop"}),
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();
        end_session(explicit);

        let idle = "test-observer-idle-KL12";
        begin_tool_call(
            "click",
            &serde_json::json!({"session": idle}),
            true,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
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
            SessionClientKind::PythonSdk,
        )
        .unwrap();

        let control = "test-observer-control-OP56";
        begin_tool_call(
            "click",
            &serde_json::json!({"session": control}),
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();
        fire_session_end(control);

        let runtime_owned = "test-observer-runtime-owned-QR78";
        begin_tool_call_with_state(
            "click",
            &serde_json::json!({"session": runtime_owned}),
            true,
            SessionTransport::Daemon,
            SessionClientKind::Direct,
            Some(SessionObservationState {
                ended: false,
                capture_scope: CaptureScope::Window,
            }),
        )
        .unwrap();

        let starts = probe.starts.lock().unwrap();
        assert!(starts.iter().any(|(id, observation)| {
            id == explicit
                && observation.declaration == SessionDeclaration::StartSession
                && !observation.revived
                && observation.client_kind == SessionClientKind::Mcp
                && observation.capture_scope == CaptureScope::Desktop
        }));
        assert!(starts.iter().any(|(id, observation)| {
            id == idle
                && observation.declaration == SessionDeclaration::ImplicitFirstAction
                && observation.transport == SessionTransport::McpHttp
        }));
        assert!(starts
            .iter()
            .any(|(id, observation)| id == revived && observation.revived));
        assert!(starts.iter().any(|(id, observation)| {
            id == runtime_owned
                && observation.declaration == SessionDeclaration::ImplicitFirstAction
                && observation.capture_scope == CaptureScope::Window
        }));
        drop(starts);

        let ends = probe.ends.lock().unwrap();
        assert!(ends.iter().any(|(id, reason, cursor)| {
            id == explicit
                && *reason == SessionEndReason::Explicit
                && cursor.is_some_and(|value| {
                    value.theme == CursorThemeCategory::Default && value.active_cursor_count == 2
                })
        }));
        assert!(ends
            .iter()
            .any(|(id, reason, _)| { id == idle && *reason == SessionEndReason::IdleTimeout }));
        assert!(!ends.iter().any(|(id, _, _)| id == control));
    }
}
