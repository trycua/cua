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

pub const DEFAULT_SESSION_IDLE_TTL: Duration = Duration::from_secs(5 * 60);

type SessionEndHook = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
type SessionReviveHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCleanupFailure {
    pub hook: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCleanupReport {
    pub complete: bool,
    pub in_progress: bool,
    pub failures: Vec<SessionCleanupFailure>,
}

#[derive(Clone)]
struct RegisteredSessionEndHook {
    name: String,
    callback: SessionEndHook,
}

struct SessionCleanupProgress {
    hooks: Vec<(u64, RegisteredSessionEndHook)>,
    completed: HashSet<u64>,
    failures: Vec<SessionCleanupFailure>,
    running: bool,
}

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
pub enum CaptureModality {
    Window,
    Desktop,
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

pub fn infer_transport_metadata(owner: &str) -> (SessionTransport, SessionClientKind) {
    if owner.contains(":http-") {
        (SessionTransport::McpHttp, SessionClientKind::Mcp)
    } else if owner.contains(":mcp-") {
        (SessionTransport::McpStdio, SessionClientKind::Mcp)
    } else if owner.contains(":cli-") {
        (SessionTransport::Cli, SessionClientKind::Cli)
    } else {
        (SessionTransport::Daemon, SessionClientKind::Direct)
    }
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
    /// True only when the platform render state currently reports visible
    /// pixels for this exact session cursor.
    pub visible: bool,
    pub theme: CursorThemeCategory,
    pub motion_customized: bool,
    pub active_cursor_count: usize,
}

/// Convert platform cursor fields to fixed categories without retaining any
/// raw theme identifier or motion value.
pub fn bounded_cursor_outcome(
    observed: bool,
    enabled: bool,
    visible: bool,
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
        visible: observed && enabled && visible,
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
        capture_modality: Option<CaptureModality>,
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
type RecordingStateReader = Arc<dyn Fn(&str) -> bool + Send + Sync>;
static RECORDING_STATE_READERS: OnceLock<Mutex<HashMap<u64, RecordingStateReader>>> =
    OnceLock::new();
static NEXT_RECORDING_STATE_READER_ID: AtomicU64 = AtomicU64::new(1);

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

pub fn register_scoped_recording_state_reader(
    reader: RecordingStateReader,
) -> RecordingStateReaderRegistration {
    let id = NEXT_RECORDING_STATE_READER_ID.fetch_add(1, Ordering::Relaxed);
    RECORDING_STATE_READERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(id, reader);
    RecordingStateReaderRegistration { id }
}

pub struct RecordingStateReaderRegistration {
    id: u64,
}

impl Drop for RecordingStateReaderRegistration {
    fn drop(&mut self) {
        if let Some(readers) = RECORDING_STATE_READERS.get() {
            readers.lock().unwrap().remove(&self.id);
        }
    }
}

pub fn cursor_visible(session_id: &str) -> bool {
    CURSOR_OUTCOME_READERS
        .get()
        .map(|readers| {
            readers.lock().unwrap().values().any(|reader| {
                let outcome = reader(session_id);
                outcome.visible
            })
        })
        .unwrap_or(false)
}

pub fn recording_active(session_id: &str) -> bool {
    RECORDING_STATE_READERS
        .get()
        .map(|readers| {
            readers
                .lock()
                .unwrap()
                .values()
                .any(|reader| reader(session_id))
        })
        .unwrap_or(false)
}

/// Private per-call context. The raw caller session id never crosses into a
/// serialized observation; it is retained only until the bounded completion
/// callback updates the process-local aggregate.
pub struct SessionToolContext {
    session_id: String,
    transport: SessionTransport,
    computer_action: bool,
    capture_modality: Option<CaptureModality>,
    escalation_reason: Option<EscalationReason>,
}

impl SessionToolContext {
    pub fn complete(self, outcome: &crate::server::ToolCompletionObservation) {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_tool_completed(
                &self.session_id,
                self.transport,
                self.computer_action,
                self.capture_modality,
                self.escalation_reason,
                outcome,
            );
        }
    }
}

static SESSION_END_HOOKS: OnceLock<Mutex<HashMap<u64, RegisteredSessionEndHook>>> = OnceLock::new();
static NEXT_SESSION_END_HOOK_ID: AtomicU64 = AtomicU64::new(1);
static SESSION_CLEANUP_PROGRESS: OnceLock<Mutex<HashMap<String, SessionCleanupProgress>>> =
    OnceLock::new();
static SESSION_REVIVE_HOOKS: OnceLock<Mutex<HashMap<u64, SessionReviveHook>>> = OnceLock::new();
static NEXT_SESSION_REVIVE_HOOK_ID: AtomicU64 = AtomicU64::new(1);

/// Last-activity timestamp per live lifecycle session. Only an admitted,
/// session-requiring call that reaches dispatch refreshes this timestamp.
/// Transport close remains an immediate cleanup path; idle eviction is the
/// crash and inactivity fallback. `"default"` and empty ids are never tracked.
static SESSION_ACTIVITY: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct LifecycleRecord {
    public_label: Option<String>,
    implicit: bool,
    owner_transport: String,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    started_at: Instant,
    /// Trusted host override. Ordinary sessions use the runtime sweep's
    /// default TTL so tests and process-level configuration can still supply
    /// that fallback.
    idle_ttl: Option<Duration>,
    in_flight: usize,
    pending_end: Option<SessionEndReason>,
}

static LIFECYCLE_RECORDS: OnceLock<Mutex<HashMap<String, LifecycleRecord>>> = OnceLock::new();

fn lifecycle_records() -> &'static Mutex<HashMap<String, LifecycleRecord>> {
    LIFECYCLE_RECORDS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
pub struct LifecycleSessionSnapshot {
    pub runtime_id: String,
    pub public_label: Option<String>,
    pub implicit: bool,
    pub transport: SessionTransport,
    pub client_kind: SessionClientKind,
    pub owner_transport: String,
    pub ending: bool,
    pub started_for: Duration,
    pub idle: Duration,
    pub expires_in: Duration,
}

/// RAII protection for one admitted session-requiring dispatch. Idle eviction
/// cannot end the session while this guard is alive. Dropping it refreshes the
/// idle clock at platform-dispatch completion and completes any end that raced
/// the call.
pub struct SessionDispatchGuard {
    session_id: String,
}

impl Drop for SessionDispatchGuard {
    fn drop(&mut self) {
        let pending = {
            let mut records = lifecycle_records().lock().unwrap();
            let Some(record) = records.get_mut(&self.session_id) else {
                return;
            };
            record.in_flight = record.in_flight.saturating_sub(1);
            activity()
                .lock()
                .unwrap()
                .insert(self.session_id.clone(), Instant::now());
            (record.in_flight == 0)
                .then(|| record.pending_end.take())
                .flatten()
        };
        if let Some(reason) = pending {
            finish_session_end(&self.session_id, reason);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn begin_session_dispatch(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
) -> Result<SessionDispatchGuard, &'static str> {
    begin_session_dispatch_inner(
        session_id,
        public_label,
        owner_transport,
        implicit,
        transport,
        client_kind,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn begin_session_dispatch_with_ttl(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    idle_ttl: Duration,
) -> Result<SessionDispatchGuard, &'static str> {
    begin_session_dispatch_inner(
        session_id,
        public_label,
        owner_transport,
        implicit,
        transport,
        client_kind,
        Some(idle_ttl),
    )
}

#[allow(clippy::too_many_arguments)]
fn begin_session_dispatch_inner(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    idle_ttl: Option<Duration>,
) -> Result<SessionDispatchGuard, &'static str> {
    if !is_trackable(session_id) {
        return Err("session has ended");
    }
    let now = Instant::now();
    {
        // Keep tombstone admission and live-record insertion in one critical
        // section. Otherwise an end could land between the old pre-check and
        // insertion, leaving a tombstoned record that was silently recreated
        // by a racing first action.
        let ended = ended_sessions().lock().unwrap();
        if ended.contains_key(session_id) {
            return Err("session has ended");
        }
        let mut records = lifecycle_records().lock().unwrap();
        let record = records
            .entry(session_id.to_owned())
            .or_insert_with(|| LifecycleRecord {
                public_label: public_label.map(str::to_owned),
                implicit,
                owner_transport: owner_transport.to_owned(),
                transport,
                client_kind,
                started_at: now,
                idle_ttl,
                in_flight: 0,
                pending_end: None,
            });
        if record.owner_transport != owner_transport {
            return Err("session is not available to this transport");
        }
        if record.pending_end.is_some() {
            return Err("session is ending");
        }
        if idle_ttl.is_some() {
            record.idle_ttl = idle_ttl;
        }
        record.in_flight += 1;
    }
    activity()
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), now);
    Ok(SessionDispatchGuard {
        session_id: session_id.to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn activate_session(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
) -> Result<(), &'static str> {
    activate_session_inner(
        session_id,
        public_label,
        owner_transport,
        implicit,
        transport,
        client_kind,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn activate_session_with_ttl(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    idle_ttl: Duration,
) -> Result<(), &'static str> {
    activate_session_inner(
        session_id,
        public_label,
        owner_transport,
        implicit,
        transport,
        client_kind,
        Some(idle_ttl),
    )
}

#[allow(clippy::too_many_arguments)]
fn activate_session_inner(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    idle_ttl: Option<Duration>,
) -> Result<(), &'static str> {
    let now = Instant::now();
    {
        let mut records = lifecycle_records().lock().unwrap();
        if let Some(record) = records.get_mut(session_id) {
            if record.owner_transport != owner_transport {
                return Err("session is not available to this transport");
            }
            if record.pending_end.is_some() {
                return Err("session is ending");
            }
            if idle_ttl.is_some() {
                record.idle_ttl = idle_ttl;
            }
        } else {
            records.insert(
                session_id.to_owned(),
                LifecycleRecord {
                    public_label: public_label.map(str::to_owned),
                    implicit,
                    owner_transport: owner_transport.to_owned(),
                    transport,
                    client_kind,
                    started_at: now,
                    idle_ttl,
                    in_flight: 0,
                    pending_end: None,
                },
            );
        }
    }
    activity()
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), now);
    Ok(())
}

/// Atomically declare a lifecycle episode for one authenticated transport.
///
/// Unlike calling `revive_session_for_owner` and `activate_session` in two
/// steps, this does not leave a gap where another transport can claim a
/// recycled public label after its tombstone is cleared. Cleanup is completed
/// before the transition, then the tombstone removal and live owner binding
/// happen under one lock order.
#[allow(clippy::too_many_arguments)]
pub fn activate_or_revive_session_for_owner(
    session_id: &str,
    public_label: Option<&str>,
    owner_transport: &str,
    implicit: bool,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    idle_ttl: Option<Duration>,
) -> Result<bool, &'static str> {
    if !is_trackable(session_id) {
        return Err("session is not available to this transport");
    }

    let needs_cleanup = {
        let ended = ended_sessions().lock().unwrap();
        match ended.get(session_id) {
            Some(Some(owner)) if owner != owner_transport => {
                return Err("session is not available to this transport");
            }
            Some(None) if owner_transport != session_id => {
                return Err("session is not available to this transport");
            }
            Some(_) => true,
            None => false,
        }
    };
    if needs_cleanup && !retry_session_cleanup(session_id).complete {
        return Err("session cleanup is incomplete; retry end_session before revival");
    }

    let now = Instant::now();
    let revived = {
        let mut ended = ended_sessions().lock().unwrap();
        let revived = match ended.get(session_id) {
            Some(Some(owner)) if owner != owner_transport => {
                return Err("session is not available to this transport");
            }
            Some(None) if owner_transport != session_id => {
                return Err("session is not available to this transport");
            }
            Some(_) => true,
            None => false,
        };

        let mut records = lifecycle_records().lock().unwrap();
        if let Some(record) = records.get_mut(session_id) {
            if record.owner_transport != owner_transport || record.pending_end.is_some() {
                return Err("session is not available to this transport");
            }
            if idle_ttl.is_some() {
                record.idle_ttl = idle_ttl;
            }
        } else {
            records.insert(
                session_id.to_owned(),
                LifecycleRecord {
                    public_label: public_label.map(str::to_owned),
                    implicit,
                    owner_transport: owner_transport.to_owned(),
                    transport,
                    client_kind,
                    started_at: now,
                    idle_ttl,
                    in_flight: 0,
                    pending_end: None,
                },
            );
        }
        if revived {
            ended.remove(session_id);
        }
        revived
    };
    activity()
        .lock()
        .unwrap()
        .insert(session_id.to_owned(), now);
    Ok(revived)
}

pub fn session_snapshot(
    session_id: &str,
    owner_transport: &str,
    ttl: Duration,
) -> Option<LifecycleSessionSnapshot> {
    let record = lifecycle_records().lock().unwrap().get(session_id)?.clone();
    if record.owner_transport != owner_transport {
        return None;
    }
    let idle = session_idle_duration(session_id).unwrap_or_default();
    let ttl = record.idle_ttl.unwrap_or(ttl);
    Some(LifecycleSessionSnapshot {
        runtime_id: session_id.to_owned(),
        public_label: record.public_label.clone(),
        implicit: record.implicit,
        transport: record.transport,
        client_kind: record.client_kind,
        owner_transport: record.owner_transport.clone(),
        ending: record.pending_end.is_some(),
        started_for: record.started_at.elapsed(),
        idle,
        expires_in: ttl.saturating_sub(idle),
    })
}

fn session_owner_matches(session_id: &str, owner_transport: &str) -> bool {
    if lifecycle_records()
        .lock()
        .unwrap()
        .get(session_id)
        .is_some_and(|record| record.owner_transport == owner_transport)
    {
        return true;
    }
    ended_sessions()
        .lock()
        .unwrap()
        .get(session_id)
        .is_some_and(|owner| owner.as_deref() == Some(owner_transport))
}

/// End a lifecycle episode only when it belongs to the authenticated
/// transport. Returns `false` for unknown and foreign ids alike so callers can
/// provide one non-enumerating response.
pub fn end_session_for_owner(session_id: &str, owner_transport: &str) -> bool {
    if !session_owner_matches(session_id, owner_transport) {
        return false;
    }
    end_session(session_id);
    true
}

pub fn list_session_snapshots(
    owner_transport: &str,
    ttl: Duration,
) -> Vec<LifecycleSessionSnapshot> {
    let mut ids = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, record)| record.owner_transport == owner_transport)
        .map(|(id, record)| (record.started_at, id.clone()))
        .collect::<Vec<_>>();
    ids.sort_by_key(|(started, id)| (*started, id.clone()));
    ids.into_iter()
        .filter_map(|(_, id)| session_snapshot(&id, owner_transport, ttl))
        .collect()
}

/// Trusted local-host view scoped to one runtime generation. This is not used
/// by agent tools; the operator adapter applies redaction before serialization.
pub fn list_session_snapshots_with_prefix(
    runtime_prefix: &str,
    ttl: Duration,
) -> Vec<LifecycleSessionSnapshot> {
    let mut records = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, _)| id.starts_with(runtime_prefix))
        .map(|(id, record)| (record.started_at, id.clone(), record.clone()))
        .collect::<Vec<_>>();
    records.sort_by_key(|(started, id, _)| (*started, id.clone()));
    records
        .into_iter()
        .map(|(_, runtime_id, record)| {
            let idle = session_idle_duration(&runtime_id).unwrap_or_default();
            LifecycleSessionSnapshot {
                runtime_id,
                public_label: record.public_label,
                implicit: record.implicit,
                transport: record.transport,
                client_kind: record.client_kind,
                owner_transport: record.owner_transport,
                ending: record.pending_end.is_some(),
                started_for: record.started_at.elapsed(),
                idle,
                expires_in: record.idle_ttl.unwrap_or(ttl).saturating_sub(idle),
            }
        })
        .collect()
}

pub fn end_sessions_for_owner(owner_transport: &str, reason: SessionEndReason) -> usize {
    let ids = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, record)| record.owner_transport == owner_transport)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in &ids {
        end_session_with_reason(id, reason);
    }
    ids.len()
}

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

/// Begin observation with a runtime-owner-provided view of session
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
    let session_id = ["session", "_session_id"].into_iter().find_map(|key| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|id| is_trackable(id))
    })?;
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
    let capture_modality = capture_modality_for(tool_name, args);
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
        capture_modality,
        escalation_reason,
    })
}

fn capture_modality_for(tool_name: &str, args: &serde_json::Value) -> Option<CaptureModality> {
    if let Some(kind) = args
        .get("target")
        .and_then(serde_json::Value::as_object)
        .and_then(|target| target.get("kind"))
        .and_then(serde_json::Value::as_str)
    {
        return match kind {
            "window" => Some(CaptureModality::Window),
            "desktop" => Some(CaptureModality::Desktop),
            _ => None,
        };
    }
    if let Some(scope) = args.get("scope").and_then(serde_json::Value::as_str) {
        return match scope {
            "window" => Some(CaptureModality::Window),
            "desktop" => Some(CaptureModality::Desktop),
            _ => None,
        };
    }
    if args.get("pid").is_some_and(serde_json::Value::is_number)
        || args
            .get("window_id")
            .is_some_and(serde_json::Value::is_number)
    {
        return Some(CaptureModality::Window);
    }
    match tool_name {
        "get_window_state" => Some(CaptureModality::Window),
        "get_desktop_state" | "get_screen_size" | "get_cursor_position" => {
            Some(CaptureModality::Desktop)
        }
        _ => None,
    }
}

/// Session ids that have already had their `session_end` fired. Dedupes the
/// control-connection EOF teardown (the reaper) against any stray legacy
/// `session_end` method that a mixed-version (new proxy / old proxy) rollout
/// might still send — `fire_session_end` is the single fan-out point and must
/// be idempotent because the overlay Remove + recording stop must run exactly
/// once. Growth is bounded (one short string per ended session over the
/// daemon's lifetime); eviction is a deliberate non-blocking follow-up.
static ENDED_SESSIONS: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
/// Runtime generations that have received terminal revoke-all.
///
/// This latch is intentionally independent of grants and public session
/// labels. Once set, every later dispatch owned by the same runtime
/// generation fails closed until that runtime is destroyed.
static SUSPENDED_RUNTIME_SCOPES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn hooks() -> &'static Mutex<HashMap<u64, RegisteredSessionEndHook>> {
    SESSION_END_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cleanup_progress() -> &'static Mutex<HashMap<String, SessionCleanupProgress>> {
    SESSION_CLEANUP_PROGRESS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn revive_hooks() -> &'static Mutex<HashMap<u64, SessionReviveHook>> {
    SESSION_REVIVE_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ended_sessions() -> &'static Mutex<HashMap<String, Option<String>>> {
    ENDED_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
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

pub(crate) fn public_session_label(session_id: &str) -> &str {
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
    register_scoped_fallible_session_end_hook("session_state", move |session_id| {
        hook(session_id);
        Ok(())
    })
}

/// Register a named cleanup hook whose failure is retained for bounded retry.
/// A successful hook is never run twice for the same lifecycle episode.
pub fn register_scoped_fallible_session_end_hook(
    name: impl Into<String>,
    hook: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
) -> SessionEndHookRegistration {
    let id = NEXT_SESSION_END_HOOK_ID.fetch_add(1, Ordering::Relaxed);
    hooks().lock().unwrap().insert(
        id,
        RegisteredSessionEndHook {
            name: name.into(),
            callback: Arc::new(hook),
        },
    );
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

/// Register a runtime-owned callback for a successful explicit revival.
/// Dropping the returned guard removes the callback.
pub fn register_scoped_session_revive_hook(
    hook: impl Fn(&str) + Send + Sync + 'static,
) -> SessionReviveHookRegistration {
    let id = NEXT_SESSION_REVIVE_HOOK_ID.fetch_add(1, Ordering::Relaxed);
    revive_hooks().lock().unwrap().insert(id, Arc::new(hook));
    SessionReviveHookRegistration { id }
}

pub struct SessionReviveHookRegistration {
    id: u64,
}

impl Drop for SessionReviveHookRegistration {
    fn drop(&mut self) {
        revive_hooks().lock().unwrap().remove(&self.id);
    }
}

/// Notify session-owned subsystems after `start_session` has successfully
/// revived and rebound a recycled id. Unlike [`revive_session`], this does not
/// alter core lifecycle state; it fans the completed transition out to
/// platform-owned tombstones.
pub fn fire_session_revive(session_id: &str) {
    let owner = lifecycle_records()
        .lock()
        .unwrap()
        .get(session_id)
        .map(|record| record.owner_transport.clone());
    if let Some(owner) = owner {
        let _ = fire_session_revive_for_owner(session_id, &owner);
    }
}

/// Notify render-side owners only while the revived episode is still live and
/// owned by the same transport. Holding the tombstone lock across callbacks
/// orders a racing end after revival, so its cleanup cannot be followed by a
/// late overlay resurrection.
pub fn fire_session_revive_for_owner(session_id: &str, owner_transport: &str) -> bool {
    let registered = revive_hooks()
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let ended = ended_sessions().lock().unwrap();
    if ended.contains_key(session_id)
        || !lifecycle_records()
            .lock()
            .unwrap()
            .get(session_id)
            .is_some_and(|record| {
                record.owner_transport == owner_transport && record.pending_end.is_none()
            })
    {
        return false;
    }
    for hook in registered {
        hook(session_id);
    }
    true
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
    fire_session_end_for_owner(session_id, None)
}

fn fire_session_end_for_owner(session_id: &str, owner_transport: Option<&str>) -> bool {
    let first_fire = mark_session_ended(session_id, owner_transport);
    if first_fire {
        initialize_session_cleanup(session_id);
    }
    let _ = retry_session_cleanup(session_id);
    first_fire
}

/// Atomically remove a live lifecycle record and install its tombstone.
///
/// Hooks run after this short critical section. Keeping this transition under
/// the same lock order as dispatch admission prevents a racing first action
/// from recreating a record immediately before or after termination.
fn mark_session_ended(session_id: &str, owner_transport: Option<&str>) -> bool {
    activity().lock().unwrap().remove(session_id);
    let mut ended = ended_sessions().lock().unwrap();
    let record_owner = lifecycle_records()
        .lock()
        .unwrap()
        .remove(session_id)
        .map(|record| record.owner_transport);
    if ended.contains_key(session_id) {
        false
    } else {
        ended.insert(
            session_id.to_owned(),
            owner_transport.map(str::to_owned).or(record_owner),
        );
        true
    }
}

fn initialize_session_cleanup(session_id: &str) {
    crate::capture_scope::clear_session(session_id);
    let mut registered = hooks()
        .lock()
        .unwrap()
        .iter()
        .map(|(id, hook)| (*id, hook.clone()))
        .collect::<Vec<_>>();
    registered.sort_by_key(|(id, _)| *id);
    cleanup_progress().lock().unwrap().insert(
        session_id.to_owned(),
        SessionCleanupProgress {
            hooks: registered,
            completed: HashSet::new(),
            failures: Vec::new(),
            running: false,
        },
    );
}

/// Retry only the cleanup hooks that have not yet succeeded for this ended
/// lifecycle episode. Panics are converted to structured failures so another
/// explicit end can retry them instead of losing cleanup state.
pub fn retry_session_cleanup(session_id: &str) -> SessionCleanupReport {
    let work = {
        let mut all = cleanup_progress().lock().unwrap();
        let Some(progress) = all.get_mut(session_id) else {
            return SessionCleanupReport {
                complete: true,
                in_progress: false,
                failures: Vec::new(),
            };
        };
        if progress.running {
            return SessionCleanupReport {
                complete: false,
                in_progress: true,
                failures: progress.failures.clone(),
            };
        }
        progress.running = true;
        progress
            .hooks
            .iter()
            .filter(|(id, _)| !progress.completed.contains(id))
            .cloned()
            .collect::<Vec<_>>()
    };

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for (id, hook) in work {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (hook.callback)(session_id)));
        match result {
            Ok(Ok(())) => successes.push(id),
            Ok(Err(error)) => failures.push(SessionCleanupFailure {
                hook: hook.name,
                error,
            }),
            Err(_) => failures.push(SessionCleanupFailure {
                hook: hook.name,
                error: "cleanup hook panicked".into(),
            }),
        }
    }

    let mut all = cleanup_progress().lock().unwrap();
    let Some(progress) = all.get_mut(session_id) else {
        return SessionCleanupReport {
            complete: true,
            in_progress: false,
            failures: Vec::new(),
        };
    };
    progress.completed.extend(successes);
    progress.failures = failures;
    progress.running = false;
    let complete = progress.completed.len() == progress.hooks.len();
    let failures = progress.failures.clone();
    if complete {
        all.remove(session_id);
    }
    SessionCleanupReport {
        complete,
        in_progress: false,
        failures,
    }
}

/// Inspect cleanup progress without starting another attempt.
///
/// Lifecycle tools use this after requesting an end so one public call runs
/// each unfinished hook at most once. A later explicit `end_session` call is
/// the bounded retry boundary for hooks that failed.
pub fn session_cleanup_status(session_id: &str) -> SessionCleanupReport {
    let all = cleanup_progress().lock().unwrap();
    let Some(progress) = all.get(session_id) else {
        return SessionCleanupReport {
            complete: true,
            in_progress: false,
            failures: Vec::new(),
        };
    };
    SessionCleanupReport {
        complete: progress.completed.len() == progress.hooks.len(),
        in_progress: progress.running,
        failures: progress.failures.clone(),
    }
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
    ended.retain(|session, _| !session.starts_with(prefix));
    let forgotten = before - ended.len();
    drop(ended);
    cleanup_progress()
        .lock()
        .unwrap()
        .retain(|session, _| !session.starts_with(prefix));
    crate::capture_scope::clear_sessions_with_prefix(prefix);
    forgotten
}

/// Whether `fire_session_end` has already run for this `session_id`. The
/// daemon-side authority for "this session is permanently gone"; the macOS
/// overlay keeps its own render-side tombstone keyed on the same id.
pub fn is_session_ended(session_id: &str) -> bool {
    ended_sessions().lock().unwrap().contains_key(session_id)
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
    if !retry_session_cleanup(session_id).complete {
        return false;
    }
    ended_sessions()
        .lock()
        .unwrap()
        .remove(session_id)
        .is_some()
}

/// Owner-checked revival used by the public `start_session` tool. A public
/// label is never proof that a new transport owns a prior episode: only the
/// transport that ended the episode may revive it without a separate trusted
/// host resume proof.
pub fn revive_session_for_owner(
    session_id: &str,
    owner_transport: &str,
) -> Result<bool, &'static str> {
    if !is_trackable(session_id) {
        return Ok(false);
    }

    // Validate ownership before running any cleanup callback. A caller that
    // merely guesses another transport's public label must not be able to
    // trigger work for that lifecycle episode.
    {
        let ended = ended_sessions().lock().unwrap();
        let Some(ended_owner) = ended.get(session_id) else {
            return Ok(false);
        };
        match ended_owner.as_deref() {
            Some(owner) if owner != owner_transport => {
                return Err("session is not available to this transport");
            }
            // Legacy tombstones did not retain ownership. Allow only the
            // direct runtime shape where the trusted owner key and lifecycle
            // id coincide; transport-backed callers fail closed.
            None if owner_transport != session_id => {
                return Err("session is not available to this transport");
            }
            _ => {}
        }
    }

    if !retry_session_cleanup(session_id).complete {
        return Err("session cleanup is incomplete; retry end_session before revival");
    }

    // Recheck after cleanup because another thread may have completed the
    // revival while callbacks were running.
    let mut ended = ended_sessions().lock().unwrap();
    let Some(ended_owner) = ended.get(session_id) else {
        return Ok(false);
    };
    match ended_owner.as_deref() {
        Some(owner) if owner != owner_transport => {
            Err("session is not available to this transport")
        }
        None if owner_transport != session_id => Err("session is not available to this transport"),
        _ => {
            ended.remove(session_id);
            Ok(true)
        }
    }
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
    let defer = {
        let mut records = lifecycle_records().lock().unwrap();
        match records.get_mut(session_id) {
            Some(record) if record.in_flight > 0 => {
                record.pending_end.get_or_insert(reason);
                true
            }
            _ => false,
        }
    };
    if defer {
        return;
    }
    finish_session_end(session_id, reason);
}

fn finish_session_end(session_id: &str, reason: SessionEndReason) {
    let first_fire = mark_session_ended(session_id, None);
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
    if first_fire {
        initialize_session_cleanup(session_id);
    }
    let _ = retry_session_cleanup(session_id);
    if first_fire {
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
    let lifecycle = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .map(|(id, record)| (id.clone(), (record.in_flight, record.idle_ttl)))
        .collect::<HashMap<_, _>>();
    let stale: Vec<String> = {
        let map = activity().lock().unwrap();
        map.iter()
            .filter(|(id, last)| {
                let (in_flight, session_ttl) = lifecycle.get(*id).copied().unwrap_or_default();
                now.duration_since(**last) >= session_ttl.unwrap_or(ttl) && in_flight == 0
            })
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
    let lifecycle = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .map(|(id, record)| (id.clone(), (record.in_flight, record.idle_ttl)))
        .collect::<HashMap<_, _>>();
    let stale: Vec<String> = {
        let map = activity().lock().unwrap();
        map.iter()
            .filter(|(id, last)| {
                let (in_flight, session_ttl) = lifecycle.get(*id).copied().unwrap_or_default();
                id.starts_with(prefix)
                    && now.duration_since(**last) >= session_ttl.unwrap_or(ttl)
                    && in_flight == 0
            })
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
    use std::sync::{Arc, Barrier};

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
            _: Option<CaptureModality>,
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
        assert!(evict_idle_with_prefix(Duration::from_secs(3600), sid)
            .iter()
            .all(|s| s != sid));
        // A zero TTL treats any prior activity as idle → evicts it.
        let evicted = evict_idle_with_prefix(Duration::ZERO, sid);
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
        let evicted = evict_idle_with_prefix(Duration::ZERO, "test-anonymous-never-matches");
        assert!(!evicted.iter().any(|s| s == "default" || s.is_empty()));
    }

    #[test]
    fn cursor_outcomes_are_fixed_categories_without_raw_values() {
        let custom =
            bounded_cursor_outcome(true, true, true, Some("private.customer.theme"), true, 7);
        assert_eq!(custom.theme, CursorThemeCategory::Custom);
        assert!(custom.motion_customized);
        assert_eq!(custom.active_cursor_count, 7);
        let debug = format!("{custom:?}");
        let forbidden = "private.customer.theme";
        assert!(
            !debug.contains(forbidden),
            "cursor outcome leaked {forbidden}: {debug}"
        );

        let unknown = bounded_cursor_outcome(false, true, true, Some("cua.default"), true, 0);
        assert_eq!(unknown.theme, CursorThemeCategory::Unknown);
        assert!(!unknown.enabled);
        assert!(!unknown.visible);
        assert!(!unknown.motion_customized);
    }

    #[test]
    fn end_session_is_explicit_teardown() {
        let sid = "test-end-session-112233";
        touch_session(sid);
        end_session(sid);
        assert!(is_session_ended(sid));
        // Its TTL entry is gone, so a later sweep doesn't re-fire for it.
        assert!(!evict_idle_with_prefix(Duration::ZERO, sid)
            .iter()
            .any(|s| s == sid));
    }

    #[test]
    fn lifecycle_identity_is_scoped_to_its_transport_owner() {
        let sid = "test-owner-scope-session-A1B2C3";
        let owner_a = "test-owner-transport-a";
        let owner_b = "test-owner-transport-b";
        assert!(activate_session(
            sid,
            Some("shared-public-label"),
            owner_a,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
        )
        .is_ok());

        assert!(session_snapshot(sid, owner_a, DEFAULT_SESSION_IDLE_TTL).is_some());
        assert!(session_snapshot(sid, owner_b, DEFAULT_SESSION_IDLE_TTL).is_none());
        assert!(activate_session(
            sid,
            Some("shared-public-label"),
            owner_b,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
        )
        .is_err());
        assert!(begin_session_dispatch(
            sid,
            Some("shared-public-label"),
            owner_b,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
        )
        .is_err());
        assert!(!end_session_for_owner(sid, owner_b));
        assert!(!is_session_ended(sid));

        assert!(end_session_for_owner(sid, owner_a));
        assert!(is_session_ended(sid));
        assert!(revive_session_for_owner(sid, owner_b).is_err());
        assert!(is_session_ended(sid));
        assert_eq!(revive_session_for_owner(sid, owner_a), Ok(true));
    }

    #[test]
    fn atomic_start_cannot_transfer_an_ended_label_to_another_transport() {
        let sid = "test-atomic-owner-revival-B1C2D3";
        let owner_a = "test-atomic-owner-a";
        let owner_b = "test-atomic-owner-b";
        activate_session(
            sid,
            Some("recycled-label"),
            owner_a,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
        )
        .unwrap();
        assert!(end_session_for_owner(sid, owner_a));

        assert!(activate_or_revive_session_for_owner(
            sid,
            Some("recycled-label"),
            owner_b,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
            None,
        )
        .is_err());
        assert!(is_session_ended(sid));

        assert_eq!(
            activate_or_revive_session_for_owner(
                sid,
                Some("recycled-label"),
                owner_a,
                false,
                SessionTransport::McpHttp,
                SessionClientKind::Mcp,
                None,
            ),
            Ok(true)
        );
        assert!(session_snapshot(sid, owner_a, DEFAULT_SESSION_IDLE_TTL).is_some());
        assert!(session_snapshot(sid, owner_b, DEFAULT_SESSION_IDLE_TTL).is_none());
        assert!(end_session_for_owner(sid, owner_a));
    }

    #[test]
    fn foreign_revival_does_not_run_an_owners_pending_cleanup() {
        let sid = "test-owner-cleanup-scope-B2C3D4";
        let owner_a = "test-owner-cleanup-transport-a";
        let owner_b = "test-owner-cleanup-transport-b";
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let calls_for_hook = cleanup_calls.clone();
        let _registration =
            register_scoped_fallible_session_end_hook("owner-cleanup-test", move |ended| {
                if ended != sid {
                    return Ok(());
                }
                let call = calls_for_hook.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    Err("retry me".into())
                } else {
                    Ok(())
                }
            });

        activate_session(
            sid,
            Some("owner-cleanup"),
            owner_a,
            false,
            SessionTransport::McpHttp,
            SessionClientKind::Mcp,
        )
        .unwrap();
        assert!(end_session_for_owner(sid, owner_a));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);

        assert!(revive_session_for_owner(sid, owner_b).is_err());
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            1,
            "a foreign caller must not trigger cleanup callbacks"
        );

        assert_eq!(revive_session_for_owner(sid, owner_a), Ok(true));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn concurrent_first_dispatches_share_one_lifecycle_record() {
        let sid = "test-concurrent-first-session-D4E5F6";
        let owner = "test-concurrent-first-owner";
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let guard = begin_session_dispatch(
                    sid,
                    None,
                    owner,
                    true,
                    SessionTransport::McpStdio,
                    SessionClientKind::Mcp,
                )
                .expect("same owner may join its implicit lifecycle");
                barrier.wait();
                drop(guard);
            }));
        }
        barrier.wait();
        barrier.wait();
        assert_eq!(
            list_session_snapshots(owner, DEFAULT_SESSION_IDLE_TTL)
                .into_iter()
                .filter(|snapshot| snapshot.runtime_id == sid)
                .count(),
            1
        );
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(end_session_for_owner(sid, owner));
    }

    #[test]
    fn end_waits_for_in_flight_dispatch_and_cleanup_runs_once() {
        let sid = "test-in-flight-end-session-F7A8B9";
        let owner = "test-in-flight-end-owner";
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls_for_hook = cleanup_calls.clone();
        let _registration = register_scoped_session_end_hook(move |ended| {
            if ended == sid {
                cleanup_calls_for_hook.fetch_add(1, Ordering::SeqCst);
            }
        });
        let guard = begin_session_dispatch(
            sid,
            None,
            owner,
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();

        activate_session(
            sid,
            None,
            owner,
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .expect("idempotent start must preserve the live dispatch record");

        assert!(end_session_for_owner(sid, owner));
        let snapshot = session_snapshot(sid, owner, DEFAULT_SESSION_IDLE_TTL).unwrap();
        assert!(snapshot.ending);
        assert!(!is_session_ended(sid));
        assert!(!evict_idle_with_prefix(Duration::ZERO, sid)
            .iter()
            .any(|id| id == sid));

        drop(guard);
        assert!(is_session_ended(sid));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(end_session_for_owner(sid, owner));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn trusted_idle_ttl_override_controls_snapshot_and_eviction() {
        let sid = "test-trusted-lifecycle-ttl-A9B0C1";
        let owner = "test-trusted-lifecycle-ttl-owner";
        activate_session_with_ttl(
            sid,
            Some("trusted-ttl"),
            owner,
            false,
            SessionTransport::Daemon,
            SessionClientKind::PythonSdk,
            Duration::from_millis(1),
        )
        .unwrap();
        let snapshot = session_snapshot(sid, owner, Duration::from_secs(3600)).unwrap();
        assert!(snapshot.expires_in <= Duration::from_millis(1));
        let operator_snapshot = list_session_snapshots_with_prefix(
            "test-trusted-lifecycle-ttl-",
            Duration::from_secs(3600),
        )
        .into_iter()
        .find(|snapshot| snapshot.runtime_id == sid)
        .unwrap();
        assert!(operator_snapshot.expires_in <= Duration::from_millis(1));

        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            evict_idle_with_prefix(Duration::from_secs(3600), sid),
            vec![sid.to_owned()],
            "the trusted per-session TTL must override the runtime fallback"
        );
    }

    #[test]
    fn partial_cleanup_retries_only_failed_hooks_then_allows_revival() {
        let sid = "test-partial-cleanup-retry-C8D9E0";
        let retryable_calls = Arc::new(AtomicUsize::new(0));
        let retryable_for_hook = retryable_calls.clone();
        let _retryable =
            register_scoped_fallible_session_end_hook("retryable-test-hook", move |ended| {
                if ended != sid {
                    return Ok(());
                }
                let attempt = retryable_for_hook.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err("synthetic first-attempt failure".into())
                } else {
                    Ok(())
                }
            });
        let successful_calls = Arc::new(AtomicUsize::new(0));
        let successful_for_hook = successful_calls.clone();
        let _successful = register_scoped_session_end_hook(move |ended| {
            if ended == sid {
                successful_for_hook.fetch_add(1, Ordering::SeqCst);
            }
        });

        touch_session(sid);
        end_session(sid);
        assert!(is_session_ended(sid));
        assert_eq!(retryable_calls.load(Ordering::SeqCst), 1);
        assert_eq!(successful_calls.load(Ordering::SeqCst), 1);

        let report = retry_session_cleanup(sid);
        assert!(report.complete);
        assert!(report.failures.is_empty());
        assert_eq!(retryable_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            successful_calls.load(Ordering::SeqCst),
            1,
            "successful cleanup hooks must not run twice"
        );
        assert!(revive_session(sid));
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
    fn tool_context_accepts_trusted_implicit_session_and_uses_fixed_action_classes() {
        let _ = probe_observer();
        assert!(begin_tool_call(
            "click",
            &serde_json::json!({"_session_id": "private-fallback"}),
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .is_some());
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

        assert_eq!(
            capture_modality_for(
                "click",
                &serde_json::json!({
                    "target": {"kind": "window", "pid": 7, "window_id": 9}
                }),
            ),
            Some(CaptureModality::Window)
        );
        assert_eq!(
            capture_modality_for(
                "click",
                &serde_json::json!({
                    "target": {"kind": "desktop", "display_id": "primary"}
                }),
            ),
            Some(CaptureModality::Desktop)
        );
    }

    #[test]
    fn observer_distinguishes_explicit_idle_revival_and_control_cleanup() {
        let probe = probe_observer();
        let _ = set_cursor_outcome_reader(Arc::new(|_| {
            bounded_cursor_outcome(true, true, true, Some("cua.default"), false, 2)
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
