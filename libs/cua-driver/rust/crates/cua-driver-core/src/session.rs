// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
    reason: SessionEndReason,
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

#[derive(Debug, Clone)]
struct LifecycleRecord {
    public_label: Option<String>,
    implicit: bool,
    owner_transport: String,
    transport: SessionTransport,
    client_kind: SessionClientKind,
    started_at: Instant,
    /// Last use. Admission, declaration and dispatch completion stamp it
    /// with `Instant::now()` while holding the records lock, so stamps only
    /// move forward and the idle sweep reads it with `in_flight` in one
    /// critical section.
    last_activity: Instant,
    /// Trusted host override. Ordinary sessions use the runtime sweep's
    /// default TTL so tests and process-level configuration can still supply
    /// that fallback.
    idle_ttl: Option<Duration>,
    in_flight: usize,
    pending_end: Option<SessionEndReason>,
    /// Whether an `end_session` asked for the pending end. A transport close
    /// or the idle sweep never sets it, whatever its reason.
    pending_explicit_end: bool,
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
    pub idle_ttl: Duration,
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
            record.last_activity = Instant::now();
            (record.in_flight == 0)
                .then(|| {
                    record.pending_end.take().map(|reason| {
                        (
                            reason,
                            record.pending_explicit_end,
                            record.owner_transport.clone(),
                        )
                    })
                })
                .flatten()
        };
        // Bound to this episode's owner: if another end and a new owner's
        // claim land before this finish, the new episode is left alone.
        if let Some((reason, explicit, owner)) = pending {
            finish_session_end(&self.session_id, reason, explicit, Some(&owner), None);
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
    // An unnamed transport session reclaimed by the idle sweep starts a new
    // lifecycle episode on its owner's next call instead of refusing every
    // later call, and so does a label whose owner transport has closed, for
    // any other transport, unless the label was ended with `end_session`. Its
    // cleanup must have finished first, so snapshots, element tokens, grants,
    // and recordings from the ended episode stay retired.
    let recreate = recreates_on_next_call(session_id, owner_transport);
    if recreate && !retry_session_cleanup(session_id).complete {
        return Err("session cleanup is incomplete; retry the call");
    }
    let now = Instant::now();
    let recreated = {
        // Keep tombstone admission and live-record insertion in one critical
        // section. Otherwise an end could land between the old pre-check and
        // insertion, leaving a tombstoned record that was silently recreated
        // by a racing first action.
        let mut ended = ended_sessions().lock().unwrap();
        let mut idle_ended = idle_ended_sessions().lock().unwrap();
        let recreated = ended.contains_key(session_id);
        if recreated
            && !(recreate
                && ((idle_ended.get(session_id) == Some(&IdleRevival::NextCall)
                    && ended
                        .get(session_id)
                        .is_some_and(|owner| owner.as_deref() == Some(owner_transport)))
                    || released_to_next_call(&ended, session_id, owner_transport)))
        {
            return Err("session has ended");
        }
        if recreated && cleanup_pending(session_id) {
            return Err("session cleanup is incomplete; retry the call");
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
                last_activity: Instant::now(),
                idle_ttl,
                in_flight: 0,
                pending_end: None,
                pending_explicit_end: false,
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
        record.last_activity = Instant::now();
        if recreated {
            remove_tombstone(&mut ended, &records, session_id);
            idle_ended.remove(session_id);
        }
        recreated
    };
    if recreated {
        // Platform overlays keep their own late-command tombstones; clear
        // them exactly as an explicit start_session revival would.
        fire_session_revive_for_owner(session_id, owner_transport);
    }
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
            record.last_activity = Instant::now();
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
                    last_activity: Instant::now(),
                    idle_ttl,
                    in_flight: 0,
                    pending_end: None,
                    pending_explicit_end: false,
                },
            );
        }
    }
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
            Some(Some(_)) if claim_refused(&ended, session_id, owner_transport) => {
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
            Some(Some(_)) if claim_refused(&ended, session_id, owner_transport) => {
                return Err("session is not available to this transport");
            }
            Some(None) if owner_transport != session_id => {
                return Err("session is not available to this transport");
            }
            Some(_) => true,
            None => false,
        };
        if revived && cleanup_pending(session_id) {
            return Err("session cleanup is incomplete; retry end_session before revival");
        }

        let mut records = lifecycle_records().lock().unwrap();
        if let Some(record) = records.get_mut(session_id) {
            if record.owner_transport != owner_transport || record.pending_end.is_some() {
                return Err("session is not available to this transport");
            }
            if idle_ttl.is_some() {
                record.idle_ttl = idle_ttl;
            }
            record.last_activity = Instant::now();
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
                    last_activity: Instant::now(),
                    idle_ttl,
                    in_flight: 0,
                    pending_end: None,
                    pending_explicit_end: false,
                },
            );
        }
        if revived {
            remove_tombstone(&mut ended, &records, session_id);
            idle_ended_sessions().lock().unwrap().remove(session_id);
        }
        revived
    };
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
    let idle = record.last_activity.elapsed();
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
        idle_ttl: ttl,
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
    // Bound to the checked owner, so a release and another transport's claim
    // landing between the check and the end leave the new episode alone.
    end_session_with_reason(
        session_id,
        SessionEndReason::Explicit,
        true,
        Some(owner_transport),
    );
    true
}

/// End every driver session a trusted transport adapter's session owns,
/// keyed the way [`crate::tool::ToolRegistry::invoke_from_trusted_adapter`]
/// keys them.
///
/// An in-process host (cua-spacesd's `/mcp`) names its transport sessions
/// with public ids such as `agent-<run>` or an `Mcp-Session-Id`, but registry
/// dispatch keys every session it owns, and so every agent cursor, by the
/// runtime-private `__cua_runtime_<generation>:<id>`. Ending the bare id would
/// fire the cleanup hooks for a key no cursor uses, leaving the overlay drawn
/// until it idled out. Returns how many sessions ended; a transport that never
/// dispatched a session-owning call ends nothing and leaves no tombstone.
pub fn end_trusted_adapter_transport(transport_session: &str) -> usize {
    if !is_trackable(transport_session) {
        return 0;
    }
    let Ok(registry) = crate::session_authorization::configured_registry() else {
        return 0;
    };
    end_sessions_for_owner(
        &registry.runtime_session_key(transport_session),
        SessionEndReason::Explicit,
    )
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
            let idle = record.last_activity.elapsed();
            let idle_ttl = record.idle_ttl.unwrap_or(ttl);
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
                idle_ttl,
                expires_in: idle_ttl.saturating_sub(idle),
            }
        })
        .collect()
}

/// End every session an owner transport holds because that transport has
/// closed. The labels it owned are released once their episodes end: another
/// transport's next call starts a fresh episode, except on a label it ended
/// with `end_session` earlier, which any other transport may claim only with
/// `start_session`. The closed transport itself stays refused. These ends are
/// never explicit, whatever `reason` the transport reports.
pub fn end_sessions_for_owner(owner_transport: &str, reason: SessionEndReason) -> usize {
    let mut ids = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, record)| record.owner_transport == owner_transport)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    ids.extend(dormant_sessions(|_, owner| owner == Some(owner_transport)));
    // Each end is bound to this owner, so an overlapping close of the same
    // transport cannot end an episode another transport has since claimed.
    for id in &ids {
        end_session_with_reason(id, reason, false, Some(owner_transport));
    }
    // Release the labels only once every episode above has ended. Released
    // earlier, another transport could claim a label in between and the loop
    // would end that new episode.
    if is_trackable(owner_transport) {
        let ended = ended_sessions().lock().unwrap();
        let records = lifecycle_records().lock().unwrap();
        if names_labels_of(&ended, &records, owner_transport) {
            closed_transports()
                .lock()
                .unwrap()
                .insert(owner_transport.to_owned());
        }
    }
    ids.len()
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
///
/// A tombstone also refuses later calls on its id, so a stray late action
/// cannot recreate session-owned state. A `start_session` from the owner
/// transport lifts it, and so does the owner's next call after the idle sweep
/// reclaimed its own unnamed session. Once the owner transport has closed,
/// its labels are free: another transport's next call starts a fresh episode,
/// except on a label ended with `end_session`, which stays refused until a
/// `start_session` from any transport.
static ENDED_SESSIONS: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
/// Tombstoned sessions whose episode ended only because the idle sweep
/// reclaimed it. They are dormant: a later explicit, transport or revoke end
/// makes the end terminal and runs cleanup again with that reason. Mutated
/// only while holding the `ENDED_SESSIONS` lock, and meaningful only while the
/// matching tombstone exists.
static IDLE_ENDED_SESSIONS: OnceLock<Mutex<HashMap<String, IdleRevival>>> = OnceLock::new();

/// How a dormant session comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleRevival {
    /// The transport's own unnamed session: its next call recreates it.
    NextCall,
    /// Only an explicit `start_session` by its owner revives it.
    StartSession,
}
/// Tombstoned sessions ended with `end_session` (the tool or an operator
/// revocation) rather than by their transport closing or the idle sweep.
/// Such a label stays refused for ordinary calls until a `start_session`,
/// also after its owner transport has closed. Mutated only while holding the
/// `ENDED_SESSIONS` lock, and meaningful only while the matching tombstone
/// exists.
static EXPLICITLY_ENDED_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
/// Owner transports whose connection has closed ([`end_sessions_for_owner`])
/// while a labelled episode, live or tombstoned, still names them. Their
/// labels are released: another transport's next call starts a fresh episode
/// instead of inheriting a refusal it cannot recover from, and its
/// `start_session` claims a label ended with `end_session`. The closed owner
/// itself stays refused, so a late call from the dead connection resurrects
/// nothing. An owner is forgotten once no labelled episode names it, so the
/// set never outgrows the tombstones and live episodes. Mutated and read only
/// while holding the `ENDED_SESSIONS` lock.
static CLOSED_TRANSPORTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
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

fn idle_ended_sessions() -> &'static Mutex<HashMap<String, IdleRevival>> {
    IDLE_ENDED_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn explicitly_ended_sessions() -> &'static Mutex<HashSet<String>> {
    EXPLICITLY_ENDED_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn closed_transports() -> &'static Mutex<HashSet<String>> {
    CLOSED_TRANSPORTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether `owner_transport` may claim the tombstoned `session_id` with
/// `start_session` because the transport that owned it has closed. Only a
/// labelled episode is released (never a transport's own unnamed id), and
/// never to the closed owner itself. The caller holds the `ENDED_SESSIONS`
/// lock and passes its map.
fn released_to(
    ended: &HashMap<String, Option<String>>,
    session_id: &str,
    owner_transport: &str,
) -> bool {
    let Some(Some(owner)) = ended.get(session_id) else {
        return false;
    };
    owner != session_id
        && owner != owner_transport
        && closed_transports().lock().unwrap().contains(owner)
}

/// Whether `owner_transport` may not claim the tombstoned `session_id`: the
/// tombstone names another live transport, or names `owner_transport` after
/// it closed. A closed transport never comes back, so a late call from it
/// must not revive what its close ended. The caller holds the
/// `ENDED_SESSIONS` lock and passes its map.
fn claim_refused(
    ended: &HashMap<String, Option<String>>,
    session_id: &str,
    owner_transport: &str,
) -> bool {
    match ended.get(session_id) {
        Some(Some(owner)) if owner == owner_transport => {
            closed_transports().lock().unwrap().contains(owner)
        }
        Some(Some(_)) => !released_to(ended, session_id, owner_transport),
        _ => false,
    }
}

/// [`released_to`] for an ordinary call, which starts a fresh episode on the
/// label. A label ended with `end_session` stays refused until a
/// `start_session` claims it.
fn released_to_next_call(
    ended: &HashMap<String, Option<String>>,
    session_id: &str,
    owner_transport: &str,
) -> bool {
    released_to(ended, session_id, owner_transport)
        && !explicitly_ended_sessions()
            .lock()
            .unwrap()
            .contains(session_id)
}

/// Whether a labelled episode, live or tombstoned, still names `owner` as its
/// transport. Its own unnamed id never counts, because it is never released.
fn names_labels_of(
    ended: &HashMap<String, Option<String>>,
    records: &HashMap<String, LifecycleRecord>,
    owner: &str,
) -> bool {
    ended
        .iter()
        .any(|(id, ended_owner)| id != owner && ended_owner.as_deref() == Some(owner))
        || records
            .iter()
            .any(|(id, record)| id != owner && record.owner_transport == owner)
}

/// Drop a tombstone that a new episode or a revival replaces, and forget its
/// closed owner once no labelled episode names it. Returns whether a
/// tombstone existed. The caller holds the `ENDED_SESSIONS` and lifecycle
/// record locks and passes both maps.
fn remove_tombstone(
    ended: &mut HashMap<String, Option<String>>,
    records: &HashMap<String, LifecycleRecord>,
    session_id: &str,
) -> bool {
    explicitly_ended_sessions()
        .lock()
        .unwrap()
        .remove(session_id);
    let Some(owner) = ended.remove(session_id) else {
        return false;
    };
    if let Some(owner) = owner {
        if !names_labels_of(ended, records, &owner) {
            closed_transports().lock().unwrap().remove(&owner);
        }
    }
    true
}

/// Dormant session ids whose tombstone owner passes `keep`.
fn dormant_sessions(keep: impl Fn(&str, Option<&str>) -> bool) -> Vec<String> {
    let ended = ended_sessions().lock().unwrap();
    idle_ended_sessions()
        .lock()
        .unwrap()
        .keys()
        .filter(|id| keep(id, ended.get(*id).and_then(Option::as_deref)))
        .cloned()
        .collect()
}

/// Whether an ended session still has cleanup to run. Revival checks this
/// under the tombstone lock, so a terminal end that has started its cleanup
/// cannot be overtaken by a revival whose new launches that cleanup would
/// then erase.
fn cleanup_pending(session_id: &str) -> bool {
    cleanup_progress().lock().unwrap().contains_key(session_id)
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

/// Release process-global session storage after the standalone daemon has
/// stopped all connection tasks and destroyed its only SDK runtime.
#[doc(hidden)]
pub fn release_process_state_for_shutdown() {
    if let Some(readers) = CURSOR_OUTCOME_READERS.get() {
        let mut readers = readers.lock().unwrap();
        readers.clear();
        readers.shrink_to_fit();
    }
    if let Some(readers) = RECORDING_STATE_READERS.get() {
        let mut readers = readers.lock().unwrap();
        readers.clear();
        readers.shrink_to_fit();
    }
    if let Some(registered) = SESSION_END_HOOKS.get() {
        let mut registered = registered.lock().unwrap();
        registered.clear();
        registered.shrink_to_fit();
    }
    if let Some(registered) = SESSION_REVIVE_HOOKS.get() {
        let mut registered = registered.lock().unwrap();
        registered.clear();
        registered.shrink_to_fit();
    }
    if let Some(progress) = SESSION_CLEANUP_PROGRESS.get() {
        let mut progress = progress.lock().unwrap();
        progress.clear();
        progress.shrink_to_fit();
    }
    if let Some(records) = LIFECYCLE_RECORDS.get() {
        let mut records = records.lock().unwrap();
        records.clear();
        records.shrink_to_fit();
    }
    if let Some(ended) = ENDED_SESSIONS.get() {
        let mut ended = ended.lock().unwrap();
        ended.clear();
        ended.shrink_to_fit();
        if let Some(idle_ended) = IDLE_ENDED_SESSIONS.get() {
            let mut idle_ended = idle_ended.lock().unwrap();
            idle_ended.clear();
            idle_ended.shrink_to_fit();
        }
        if let Some(explicitly_ended) = EXPLICITLY_ENDED_SESSIONS.get() {
            let mut explicitly_ended = explicitly_ended.lock().unwrap();
            explicitly_ended.clear();
            explicitly_ended.shrink_to_fit();
        }
        if let Some(closed) = CLOSED_TRANSPORTS.get() {
            let mut closed = closed.lock().unwrap();
            closed.clear();
            closed.shrink_to_fit();
        }
    }
    if let Some(scopes) = SUSPENDED_RUNTIME_SCOPES.get() {
        let mut scopes = scopes.lock().unwrap();
        scopes.clear();
        scopes.shrink_to_fit();
    }
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
    let transition = mark_session_ended(
        session_id,
        owner_transport,
        SessionEndReason::Unknown,
        false,
        None,
        None,
    );
    let _ = retry_session_cleanup(session_id);
    transition == EndTransition::First
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndTransition {
    /// The idle sweep's candidate was used, or ended, after it was picked, or
    /// an owner-bound end found the episode held by another transport, or a
    /// call was admitted after the end decided; that call finishes it.
    StillLive,
    /// The episode's first end.
    First,
    /// A terminal end of a session the idle sweep had left dormant.
    TerminalAfterIdle,
    /// Already ended; nothing new to clean up.
    Repeat,
}

/// Atomically remove a live lifecycle record, install its tombstone, and
/// register the cleanup the transition needs.
///
/// Hooks run after this short critical section. Keeping this transition under
/// the same lock order as dispatch admission prevents a racing first action
/// from recreating a record immediately before or after termination, and a
/// revival from slipping in before the cleanup it would have to wait for.
fn mark_session_ended(
    session_id: &str,
    owner_transport: Option<&str>,
    reason: SessionEndReason,
    explicit: bool,
    bound_to: Option<&str>,
    idle: Option<IdleCheck>,
) -> EndTransition {
    let mut ended = ended_sessions().lock().unwrap();
    let record = {
        let mut records = lifecycle_records().lock().unwrap();
        // An end bound to an owner ends only an episode that owner still
        // holds, live or dormant.
        if let Some(bound) = bound_to {
            let held = match records.get(session_id) {
                Some(record) => record.owner_transport == bound,
                None => ended
                    .get(session_id)
                    .is_some_and(|owner| owner.as_deref() == Some(bound)),
            };
            if !held {
                return EndTransition::StillLive;
            }
        }
        // The sweep's idle decision and the removal share this critical
        // section, so a call that started and finished after the sweep picked
        // the session keeps it alive.
        if idle.is_some_and(|idle| !records.get(session_id).is_some_and(|r| idle.expired(r))) {
            return EndTransition::StillLive;
        }
        // A call admitted after the end released this lock owns the session
        // now; its guard drop finishes the end.
        if let Some(record) = records.get_mut(session_id).filter(|r| r.in_flight > 0) {
            let pending = record.pending_end.get_or_insert(reason);
            if *pending == SessionEndReason::IdleTimeout {
                *pending = reason;
            }
            record.pending_explicit_end |= explicit;
            return EndTransition::StillLive;
        }
        records.remove(session_id)
    };
    let mut idle_ended = idle_ended_sessions().lock().unwrap();
    let mut explicitly_ended = explicitly_ended_sessions().lock().unwrap();
    let transition = if ended.contains_key(session_id) {
        // A later end_session also keeps the label from being released.
        if explicit {
            explicitly_ended.insert(session_id.to_owned());
        }
        if reason != SessionEndReason::IdleTimeout && idle_ended.remove(session_id).is_some() {
            EndTransition::TerminalAfterIdle
        } else {
            EndTransition::Repeat
        }
    } else {
        // Only the transport's own unnamed session, reclaimed for inactivity,
        // is recreated on its next call. Named sessions keep the resurrection
        // guard until an explicit start_session, except that a label not
        // ended with end_session is released to other transports once its
        // owner transport closes.
        let next_call = record
            .as_ref()
            .is_some_and(|record| record.implicit && record.owner_transport == session_id);
        ended.insert(
            session_id.to_owned(),
            owner_transport
                .map(str::to_owned)
                .or(record.map(|record| record.owner_transport)),
        );
        if reason == SessionEndReason::IdleTimeout {
            let revival = if next_call {
                IdleRevival::NextCall
            } else {
                IdleRevival::StartSession
            };
            idle_ended.insert(session_id.to_owned(), revival);
        } else {
            idle_ended.remove(session_id);
        }
        if explicit {
            explicitly_ended.insert(session_id.to_owned());
        } else {
            explicitly_ended.remove(session_id);
        }
        EndTransition::First
    };
    if transition != EndTransition::Repeat {
        initialize_session_cleanup(session_id, reason);
    }
    transition
}

/// A terminal end after an idle end reruns every hook with its own reason.
/// An idle run still in flight keeps `running` and picks up the rerun.
fn initialize_session_cleanup(session_id: &str, reason: SessionEndReason) {
    let mut registered = hooks()
        .lock()
        .unwrap()
        .iter()
        .map(|(id, hook)| (*id, hook.clone()))
        .collect::<Vec<_>>();
    registered.sort_by_key(|(id, _)| *id);
    let mut all = cleanup_progress().lock().unwrap();
    let progress = all
        .entry(session_id.to_owned())
        .or_insert_with(|| SessionCleanupProgress {
            reason,
            hooks: Vec::new(),
            completed: HashSet::new(),
            failures: Vec::new(),
            running: false,
        });
    progress.reason = reason;
    progress.hooks = registered;
    progress.completed.clear();
    progress.failures.clear();
}

/// Retry only the cleanup hooks that have not yet succeeded for this ended
/// lifecycle episode. Panics are converted to structured failures so another
/// explicit end can retry them instead of losing cleanup state.
pub fn retry_session_cleanup(session_id: &str) -> SessionCleanupReport {
    let (mut reason, mut work) = {
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
        (
            progress.reason,
            progress
                .hooks
                .iter()
                .filter(|(id, _)| !progress.completed.contains(id))
                .cloned()
                .collect::<Vec<_>>(),
        )
    };

    // Part of the tracked cleanup, so a revival waiting on it cannot have
    // its new capture policy cleared by this end.
    crate::capture_scope::clear_session(session_id);
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    let mut all = loop {
        for (id, hook) in work {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (hook.callback)(session_id)
            }));
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
        let all = cleanup_progress().lock().unwrap();
        match all.get(session_id) {
            // A terminal end arrived while these idle hooks ran: run its
            // hooks before anyone may revive the session.
            Some(progress) if progress.reason != reason => {
                reason = progress.reason;
                work = progress.hooks.clone();
                successes.clear();
                failures.clear();
            }
            _ => break all,
        }
    };
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

/// Why the session ended, while its cleanup hooks run or have unfinished
/// work. A hook reads this to keep state that outlives an idle end; a later
/// terminal end runs the hooks again with its own reason.
pub fn session_end_reason(session_id: &str) -> Option<SessionEndReason> {
    cleanup_progress()
        .lock()
        .unwrap()
        .get(session_id)
        .map(|progress| progress.reason)
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
    let mut sessions: Vec<String> = lifecycle_records()
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    sessions.extend(dormant_sessions(|_, _| true));
    for session in &sessions {
        end_session(session);
    }
    sessions.len()
}

/// Revoke only sessions owned by one runtime-private namespace.
pub fn revoke_sessions_with_prefix(prefix: &str) -> usize {
    let mut sessions: Vec<String> = lifecycle_records()
        .lock()
        .unwrap()
        .keys()
        .filter(|session| session.starts_with(prefix))
        .cloned()
        .collect();
    sessions.extend(dormant_sessions(|session, _| session.starts_with(prefix)));
    for session in &sessions {
        end_session(session);
    }
    sessions.len()
}

/// Forget terminal tombstones owned by a runtime generation that is itself
/// being destroyed. Live runtimes must retain tombstones until a revival or a
/// released label's next episode lifts them (see `ENDED_SESSIONS`); otherwise
/// an operator revocation could be bypassed by reusing the same public session
/// label on another transport.
pub fn forget_ended_sessions_with_prefix(prefix: &str) -> usize {
    let mut ended = ended_sessions().lock().unwrap();
    let before = ended.len();
    ended.retain(|session, _| !session.starts_with(prefix));
    let forgotten = before - ended.len();
    idle_ended_sessions()
        .lock()
        .unwrap()
        .retain(|session, _| !session.starts_with(prefix));
    explicitly_ended_sessions()
        .lock()
        .unwrap()
        .retain(|session| !session.starts_with(prefix));
    closed_transports()
        .lock()
        .unwrap()
        .retain(|owner| !owner.starts_with(prefix));
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

/// Whether `owner_transport`'s next session-requiring call recreates the
/// ended lifecycle id instead of being refused: the transport's own unnamed
/// session reclaimed only by the idle sweep, or, for any other transport, a
/// label whose owner transport has since closed and that was not ended with
/// `end_session`. Every other ended id returns `false`, including a label
/// ended with `end_session` (only `start_session` claims it) and every call
/// from the closed transport itself.
pub fn recreates_on_next_call(session_id: &str, owner_transport: &str) -> bool {
    let ended = ended_sessions().lock().unwrap();
    if released_to_next_call(&ended, session_id, owner_transport) {
        return true;
    }
    if session_id != owner_transport {
        return false;
    }
    ended
        .get(session_id)
        .is_some_and(|owner| owner.as_deref() == Some(owner_transport))
        && idle_ended_sessions().lock().unwrap().get(session_id) == Some(&IdleRevival::NextCall)
}

/// Whether termination has been requested for a runtime-private lifecycle.
///
/// Long-running adapters may use this to relinquish held input before their
/// dispatch guard drops. Cleanup hooks still run only after admitted work has
/// unwound. This read-only cancellation signal neither grants authority nor
/// revives a session; callers must use the registry's private id, not a label.
pub fn is_session_ending(session_id: &str) -> bool {
    // Match admission/termination lock order and observe pending termination
    // and completed termination in one read-side critical section.
    let ended = ended_sessions().lock().unwrap();
    ended.contains_key(session_id)
        || lifecycle_records()
            .lock()
            .unwrap()
            .get(session_id)
            .is_some_and(|record| record.pending_end.is_some())
}

/// Revive a previously-ended session id by clearing its tombstone, so a fresh
/// `start_session` with a recycled id works as a caller would expect: the id
/// becomes live again and its actions stop being rejected by the resurrection
/// guard. Returns whether the id had actually been ended (i.e. was revived).
///
/// This is the deliberate, EXPLICIT counterpart to the resurrection guard. The
/// guard exists so a *stray late action* on a dead id can't silently re-create
/// session-owned state; reviving requires an explicit `start_session` re-declare
/// of the same id, which is exactly what a caller reusing an id intends. The
/// exceptions, such as a label released by its closed owner transport, are
/// listed on `ENDED_SESSIONS`. No-op for the anonymous fallback (`"default"` /
/// empty), which is never tracked.
pub fn revive_session(session_id: &str) -> bool {
    if !is_trackable(session_id) {
        return false;
    }
    if !retry_session_cleanup(session_id).complete {
        return false;
    }
    let mut ended = ended_sessions().lock().unwrap();
    if cleanup_pending(session_id) {
        return false;
    }
    idle_ended_sessions().lock().unwrap().remove(session_id);
    remove_tombstone(&mut ended, &lifecycle_records().lock().unwrap(), session_id)
}

/// Owner-checked revival used by the public `start_session` tool. A public
/// label is never proof that a new transport owns a prior episode: only the
/// transport that ended the episode may revive it without a separate trusted
/// host resume proof, unless that transport has closed and released the
/// label.
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
            Some(_) if claim_refused(&ended, session_id, owner_transport) => {
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
        Some(_) if claim_refused(&ended, session_id, owner_transport) => {
            Err("session is not available to this transport")
        }
        None if owner_transport != session_id => Err("session is not available to this transport"),
        _ if cleanup_pending(session_id) => {
            Err("session cleanup is incomplete; retry end_session before revival")
        }
        _ => {
            remove_tombstone(&mut ended, &lifecycle_records().lock().unwrap(), session_id);
            idle_ended_sessions().lock().unwrap().remove(session_id);
            Ok(true)
        }
    }
}

#[doc(hidden)]
pub fn has_session_activity(session_id: &str) -> bool {
    lifecycle_records().lock().unwrap().contains_key(session_id)
}

#[doc(hidden)]
pub fn session_idle_duration(session_id: &str) -> Option<Duration> {
    lifecycle_records()
        .lock()
        .unwrap()
        .get(session_id)
        .map(|record| record.last_activity.elapsed())
}

/// End a session explicitly (the `end_session` tool / `session end` CLI verb):
/// drop its lifecycle record and fan `fire_session_end` out to every cleanup hook
/// (overlay remove, recording stop, config-override clear). Idempotent via
/// `fire_session_end`'s dedupe. No-op for the anonymous fallback. A label
/// ended this way stays refused for ordinary calls until a `start_session`,
/// also after its owner transport closes.
pub fn end_session(session_id: &str) {
    end_session_with_reason(session_id, SessionEndReason::Explicit, true, None);
}

fn end_session_with_reason(
    session_id: &str,
    reason: SessionEndReason,
    explicit: bool,
    bound_to: Option<&str>,
) {
    if !is_trackable(session_id) {
        return;
    }
    let defer = {
        let mut records = lifecycle_records().lock().unwrap();
        match records.get_mut(session_id) {
            Some(record) if bound_to.is_some_and(|bound| bound != record.owner_transport) => {
                return;
            }
            Some(record) if record.in_flight > 0 => {
                // A terminal end overrides a pending idle end, which would
                // keep the session's launch records.
                let pending = record.pending_end.get_or_insert(reason);
                if *pending == SessionEndReason::IdleTimeout {
                    *pending = reason;
                }
                record.pending_explicit_end |= explicit;
                true
            }
            _ => false,
        }
    };
    if defer {
        return;
    }
    finish_session_end(session_id, reason, explicit, bound_to, None);
}

fn finish_session_end(
    session_id: &str,
    reason: SessionEndReason,
    explicit: bool,
    bound_to: Option<&str>,
    idle: Option<IdleCheck>,
) -> EndTransition {
    let transition = mark_session_ended(session_id, None, reason, explicit, bound_to, idle);
    if transition == EndTransition::StillLive {
        return transition;
    }
    let first_fire = transition == EndTransition::First;
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
    let _ = retry_session_cleanup(session_id);
    if first_fire {
        if let Some(observer) = SESSION_OBSERVER.get() {
            observer.on_session_ended(public_session_label(session_id), reason, cursor);
        }
    }
    transition
}

/// The idle sweep's test for one live record. Admitted work keeps a session
/// alive however old its last use is.
#[derive(Debug, Clone, Copy)]
struct IdleCheck {
    now: Instant,
    default_ttl: Duration,
}

impl IdleCheck {
    fn expired(self, record: &LifecycleRecord) -> bool {
        // A use after the sweep's `now` is fresh even under a zero TTL.
        record.in_flight == 0
            && self
                .now
                .checked_duration_since(record.last_activity)
                .is_some_and(|idle| idle >= record.idle_ttl.unwrap_or(self.default_ttl))
    }
}

/// End every session whose last activity is older than `ttl`, returning the ids
/// ended. This is the idle-TTL sweep the daemon runs periodically: a
/// caller-declared session is no longer tied to a connection's lifetime, so a
/// run that finishes (or crashes) without calling `end_session` is reclaimed
/// here instead of leaking its cursor / recording. Sessions touched within the
/// TTL are left untouched.
pub fn evict_idle(ttl: Duration) -> Vec<String> {
    evict_idle_matching(ttl, |_| true)
}

/// Runtime-scoped form of [`evict_idle`]. The namespace prefix is minted by
/// the trusted runtime and never accepted from a tool argument.
pub fn evict_idle_with_prefix(ttl: Duration, prefix: &str) -> Vec<String> {
    evict_idle_matching(ttl, |id| id.starts_with(prefix))
}

fn evict_idle_matching(ttl: Duration, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let idle = IdleCheck {
        now: Instant::now(),
        default_ttl: ttl,
    };
    let candidates = lifecycle_records()
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, record)| is_trackable(id) && keep(id) && idle.expired(record))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    candidates
        .into_iter()
        .filter(|id| {
            finish_session_end(id, SessionEndReason::IdleTimeout, false, None, Some(idle))
                != EndTransition::StillLive
        })
        .collect()
}

/// Number of live lifecycle sessions. Diagnostics only.
pub fn active_session_count() -> usize {
    lifecycle_records().lock().unwrap().len()
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

    /// One completed session-requiring call by the session's own transport.
    fn use_session(session_id: &str) -> Result<(), &'static str> {
        begin_session_dispatch(
            session_id,
            None,
            session_id,
            false,
            SessionTransport::Daemon,
            SessionClientKind::Direct,
        )
        .map(drop)
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
    fn used_session_is_evicted_by_ttl() {
        let sid = "test-ttl-session-DDEEFF";
        use_session(sid).unwrap();
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
    fn idle_sweep_keeps_a_session_used_after_it_was_picked() {
        let sid = "test-ttl-used-after-pick-5E2";
        use_session(sid).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let idle = IdleCheck {
            now: Instant::now(),
            default_ttl: Duration::from_millis(10),
        };
        assert!(idle.expired(&lifecycle_records().lock().unwrap()[sid]));
        // A whole call lands between the sweep's pick and its end.
        use_session(sid).unwrap();
        assert_eq!(
            finish_session_end(sid, SessionEndReason::IdleTimeout, false, None, Some(idle)),
            EndTransition::StillLive
        );
        assert!(!is_session_ended(sid), "a just-used session stays live");
        end_session(sid);
    }

    #[test]
    fn zero_ttl_sweep_keeps_a_session_used_after_it_was_picked() {
        let sid = "test-ttl-zero-used-after-pick-5E2";
        use_session(sid).unwrap();
        let idle = IdleCheck {
            now: Instant::now(),
            default_ttl: Duration::ZERO,
        };
        assert!(idle.expired(&lifecycle_records().lock().unwrap()[sid]));
        std::thread::sleep(Duration::from_millis(2));
        use_session(sid).unwrap();
        assert_eq!(
            finish_session_end(sid, SessionEndReason::IdleTimeout, false, None, Some(idle)),
            EndTransition::StillLive
        );
        assert!(!is_session_ended(sid));
        end_session(sid);
    }

    /// Revoke walks the lifecycle records, the same map admission writes.
    /// The old gap (a record not yet in the activity map) cannot be reached
    /// from outside one call, so this pins the walk, not the race.
    #[test]
    fn revoke_ends_a_session_declared_without_a_call() {
        let prefix = "__cua_runtime_5e2d0000000000000000000000000000:";
        let sid = format!("{prefix}declared");
        activate_session(
            &sid,
            None,
            &sid,
            false,
            SessionTransport::Daemon,
            SessionClientKind::Direct,
        )
        .unwrap();
        assert_eq!(revoke_sessions_with_prefix(prefix), 1);
        assert!(is_session_ended(&sid));
        forget_ended_sessions_with_prefix(prefix);
    }

    #[test]
    fn anonymous_ids_are_never_tracked() {
        assert!(use_session("default").is_err());
        assert!(use_session("").is_err());
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
        use_session(sid).unwrap();
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
    fn a_closed_transport_releases_its_labels_to_other_transports() {
        let label = "test-closed-owner-label-C1D2E3";
        let ended_label = "test-closed-owner-ended-C1D2E3";
        let pending_label = "test-closed-owner-pending-C1D2E3";
        let owner_a = "test-closed-owner-transport-a";
        let owner_b = "test-closed-owner-transport-b";
        let owner_c = "test-closed-owner-transport-c";
        let begin = |sid: &str, owner: &str| {
            begin_session_dispatch(
                sid,
                (sid != owner).then_some("label"),
                owner,
                sid == owner,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        let start = |sid: &str, owner: &str| {
            activate_or_revive_session_for_owner(
                sid,
                Some("label"),
                owner,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
                None,
            )
        };
        drop(begin(owner_a, owner_a).unwrap());
        drop(begin(label, owner_a).unwrap());
        drop(begin(ended_label, owner_a).unwrap());
        assert!(end_session_for_owner(ended_label, owner_a));
        // An end_session that waits for an in-flight call is just as explicit
        // when the transport closes before that call returns.
        let in_flight = begin(pending_label, owner_a).unwrap();
        assert!(end_session_for_owner(pending_label, owner_a));

        // While A is live its labels stay its own, including the one it ended.
        assert_eq!(
            begin(label, owner_b).err(),
            Some("session is not available to this transport")
        );
        assert_eq!(begin(ended_label, owner_b).err(), Some("session has ended"));
        assert_eq!(begin(ended_label, owner_a).err(), Some("session has ended"));
        assert_eq!(
            start(ended_label, owner_b),
            Err("session is not available to this transport")
        );

        assert_eq!(
            end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit),
            3
        );
        drop(in_flight);
        assert!(is_session_ended(pending_label));
        // A late call from the closed transport resurrects nothing, and its
        // own unnamed id is never released.
        assert!(!recreates_on_next_call(label, owner_a));
        assert_eq!(begin(label, owner_a).err(), Some("session has ended"));
        assert_eq!(begin(owner_a, owner_b).err(), Some("session has ended"));
        assert_eq!(
            start(ended_label, owner_a),
            Err("session is not available to this transport"),
            "the closed transport cannot start_session its old label either"
        );
        assert_eq!(
            revive_session_for_owner(ended_label, owner_a),
            Err("session is not available to this transport")
        );

        // Another transport starts a fresh episode on a label A did not end.
        assert!(recreates_on_next_call(label, owner_b));
        drop(begin(label, owner_b).expect("a released label starts fresh"));
        assert!(!is_session_ended(label));
        assert!(session_snapshot(label, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some());

        // A label A ended with end_session stays refused for ordinary calls,
        // but any other transport may now restart it with start_session.
        for ended in [ended_label, pending_label] {
            assert!(!recreates_on_next_call(ended, owner_b));
            assert_eq!(begin(ended, owner_b).err(), Some("session has ended"));
            assert!(is_session_ended(ended));
        }
        assert_eq!(start(ended_label, owner_b), Ok(true));
        assert_eq!(start(pending_label, owner_b), Ok(true));
        drop(begin(ended_label, owner_b).expect("a restarted label runs"));

        // The new owner holds them like any live transport.
        assert_eq!(
            begin(label, owner_c).err(),
            Some("session is not available to this transport")
        );
        assert!(end_session_for_owner(label, owner_b));
        assert_eq!(begin(label, owner_c).err(), Some("session has ended"));
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            2
        );
    }

    #[test]
    fn a_close_does_not_end_a_label_claimed_while_it_runs() {
        let live = "test-close-race-live-G1H2I3";
        let dormant = "test-close-race-dormant-G1H2I3";
        let owner_a = "test-close-race-transport-a";
        let owner_b = "test-close-race-transport-b";
        let begin = |sid: &str, owner: &str| {
            begin_session_dispatch(
                sid,
                Some("label"),
                owner,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        drop(begin(live, owner_a).unwrap());
        drop(begin(dormant, owner_a).unwrap());
        assert_eq!(
            evict_idle_with_prefix(Duration::ZERO, dormant),
            vec![dormant.to_owned()]
        );

        // B tries to claim the idle label while A's close is still ending its
        // sessions. Either the claim waits for the close, or the close must
        // leave B's new episode alive.
        let claimed = Arc::new(std::sync::Mutex::new(None));
        let claimed_in_hook = claimed.clone();
        let _registration =
            register_scoped_fallible_session_end_hook("close-race-test", move |ended| {
                if ended == live {
                    let guard = begin(dormant, owner_b);
                    claimed_in_hook.lock().unwrap().replace(guard.is_ok());
                }
                Ok(())
            });
        end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);

        if *claimed.lock().unwrap() == Some(true) {
            assert!(
                session_snapshot(dormant, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some(),
                "A's close ended the episode B claimed during it"
            );
        }
        drop(begin(dormant, owner_b).expect("B claims the label after A's close"));
        assert!(session_snapshot(dormant, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some());
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            1
        );
    }

    #[test]
    fn an_overlapping_close_does_not_end_a_label_claimed_between_them() {
        let live = "test-double-close-live-J4K5L6";
        let dormant = "test-double-close-dormant-J4K5L6";
        let owner_a = "test-double-close-transport-a";
        let owner_b = "test-double-close-transport-b";
        let begin = |sid: &str, owner: &str| {
            begin_session_dispatch(
                sid,
                Some("label"),
                owner,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        drop(begin(live, owner_a).unwrap());
        drop(begin(dormant, owner_a).unwrap());
        assert_eq!(
            evict_idle_with_prefix(Duration::ZERO, dormant),
            vec![dormant.to_owned()]
        );

        // A second close of A (the legacy session_end racing the EOF reaper)
        // finishes while the first is still ending A's live label, and B
        // claims the released dormant label before the first close reaches it.
        let claimed = Arc::new(std::sync::Mutex::new(None));
        let claimed_in_hook = claimed.clone();
        let _registration =
            register_scoped_fallible_session_end_hook("double-close-test", move |ended| {
                if ended == live && claimed_in_hook.lock().unwrap().is_none() {
                    end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);
                    let guard = begin(dormant, owner_b);
                    claimed_in_hook.lock().unwrap().replace(guard.is_ok());
                }
                Ok(())
            });
        end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);

        assert_eq!(*claimed.lock().unwrap(), Some(true));
        assert!(
            session_snapshot(dormant, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some(),
            "the first close ended the episode B claimed after the second close"
        );
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            1
        );
    }

    #[test]
    fn an_owner_bound_end_leaves_another_transports_episode_alone() {
        let label = "test-owner-bound-end-M7N8O9";
        let owner_a = "test-owner-bound-end-transport-a";
        let owner_b = "test-owner-bound-end-transport-b";
        let begin = |owner: &str| {
            begin_session_dispatch(
                label,
                Some("label"),
                owner,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        // A deferred end captured while A held the label finishes only after
        // A closed and B claimed it. Bound to A, it must not end B's episode.
        drop(begin(owner_a).unwrap());
        end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);
        drop(begin(owner_b).expect("B claims the released label"));
        finish_session_end(label, SessionEndReason::Explicit, true, Some(owner_a), None);
        assert!(session_snapshot(label, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some());
        end_session_with_reason(label, SessionEndReason::Explicit, true, Some(owner_a));
        assert!(session_snapshot(label, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some());
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            1
        );
    }

    #[test]
    fn an_explicit_end_checked_for_one_owner_spares_the_next_owner() {
        let label = "test-explicit-bound-end-P1Q2R3";
        let owner_a = "test-explicit-bound-end-transport-a";
        let owner_b = "test-explicit-bound-end-transport-b";
        let begin = |owner: &str| {
            begin_session_dispatch(
                label,
                Some("label"),
                owner,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        drop(begin(owner_a).unwrap());
        assert!(session_owner_matches(label, owner_a));
        // A closes and B claims the label after A's end_session passed its
        // owner check but before it ended anything.
        end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);
        drop(begin(owner_b).expect("B claims the released label"));
        end_session_with_reason(label, SessionEndReason::Explicit, true, Some(owner_a));
        assert!(session_snapshot(label, owner_b, DEFAULT_SESSION_IDLE_TTL).is_some());
        assert!(!end_session_for_owner(label, owner_a));
        assert!(end_session_for_owner(label, owner_b));
        assert!(is_session_ended(label));
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            0
        );
    }

    #[test]
    fn a_closed_transport_is_forgotten_once_no_tombstone_names_it() {
        let label = "test-forgotten-owner-label-F7A8B9";
        let ended_label = "test-forgotten-owner-ended-F7A8B9";
        let owner_a = "test-forgotten-owner-transport-a";
        let owner_b = "test-forgotten-owner-transport-b";
        let unnamed_only = "test-forgotten-owner-transport-unnamed";
        let begin = |sid: &str, owner: &str| {
            begin_session_dispatch(
                sid,
                (sid != owner).then_some("label"),
                owner,
                sid == owner,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        let closed = |owner: &str| closed_transports().lock().unwrap().contains(owner);

        // A transport that closes without a labelled episode, like a one-shot
        // CLI call, is never recorded: it has no label to release.
        drop(begin(unnamed_only, unnamed_only).unwrap());
        assert_eq!(
            end_sessions_for_owner(unnamed_only, SessionEndReason::ProcessExit),
            1
        );
        assert!(!closed(unnamed_only));

        drop(begin(owner_a, owner_a).unwrap());
        drop(begin(label, owner_a).unwrap());
        drop(begin(ended_label, owner_a).unwrap());
        assert!(end_session_for_owner(ended_label, owner_a));
        assert_eq!(
            end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit),
            2
        );
        assert!(closed(owner_a));

        // Each claim drops one of A's tombstones. A stays recorded while one
        // still names it, and is forgotten with the last.
        drop(begin(label, owner_b).unwrap());
        assert!(closed(owner_a));
        assert_eq!(
            activate_or_revive_session_for_owner(
                ended_label,
                Some("label"),
                owner_b,
                false,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
                None,
            ),
            Ok(true)
        );
        assert!(!closed(owner_a));

        // Forgetting A loosens nothing: its own unnamed id and its late calls
        // stay refused.
        assert!(is_session_ended(owner_a));
        assert_eq!(begin(owner_a, owner_b).err(), Some("session has ended"));
        assert_eq!(begin(owner_a, owner_a).err(), Some("session has ended"));
        assert_eq!(
            begin(label, owner_a).err(),
            Some("session is not available to this transport")
        );
        assert_eq!(
            end_sessions_for_owner(owner_b, SessionEndReason::ProcessExit),
            2
        );
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

        assert!(!is_session_ending(sid));
        assert!(!end_session_for_owner(sid, "unrelated-owner"));
        assert!(!is_session_ending(sid));
        assert!(end_session_for_owner(sid, owner));
        assert!(is_session_ending(sid));
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
        let snapshot = session_snapshot(sid, owner, DEFAULT_SESSION_IDLE_TTL).unwrap();
        assert!(snapshot.ending);
        assert!(!is_session_ended(sid));
        assert!(!evict_idle_with_prefix(Duration::ZERO, sid)
            .iter()
            .any(|id| id == sid));

        drop(guard);
        assert!(is_session_ended(sid));
        assert!(is_session_ending(sid));
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

        use_session(sid).unwrap();
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
    fn only_the_owners_unnamed_session_is_recreated_after_an_idle_end() {
        let pid = std::process::id();
        let implicit = format!("idle-recreate-implicit-{pid}");
        let named = format!("idle-recreate-named-{pid}");
        let named_owner = format!("idle-recreate-named-owner-{pid}");
        let begin = |id: &str, label: Option<&str>, owner: &str| {
            begin_session_dispatch(
                id,
                label,
                owner,
                label.is_none(),
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        // Fail at the idle end and at the first recreation attempt.
        let failures = Arc::new(AtomicUsize::new(2));
        let failures_for_hook = failures.clone();
        let observed = implicit.clone();
        let _hook = register_scoped_fallible_session_end_hook("idle-recreate-test", move |id| {
            if id == observed
                && failures_for_hook
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    })
                    .is_ok()
            {
                return Err("synthetic cleanup failure".into());
            }
            Ok(())
        });

        drop(begin(&implicit, None, &implicit).unwrap());
        drop(begin(&named, Some("named"), &named_owner).unwrap());
        assert_eq!(
            evict_idle_with_prefix(Duration::ZERO, &implicit),
            std::slice::from_ref(&implicit)
        );
        assert_eq!(
            evict_idle_with_prefix(Duration::ZERO, &named),
            std::slice::from_ref(&named)
        );

        // A named episode keeps its resurrection guard.
        assert!(!recreates_on_next_call(&named, &named_owner));
        assert_eq!(
            begin(&named, Some("named"), &named_owner).err(),
            Some("session has ended")
        );
        // Another transport cannot claim the unnamed id.
        assert_eq!(
            begin(&implicit, None, "another-transport").err(),
            Some("session has ended")
        );
        // Unfinished cleanup is retried by the call and blocks recreation.
        assert!(recreates_on_next_call(&implicit, &implicit));
        assert_eq!(
            begin(&implicit, None, &implicit).err(),
            Some("session cleanup is incomplete; retry the call")
        );
        let guard = begin(&implicit, None, &implicit).expect("recreated after cleanup");
        assert!(!is_session_ended(&implicit));
        assert!(
            session_snapshot(&implicit, &implicit, DEFAULT_SESSION_IDLE_TTL)
                .is_some_and(|snapshot| snapshot.implicit)
        );
        drop(guard);

        // An explicit end stays terminal until start_session.
        end_session(&implicit);
        assert!(!recreates_on_next_call(&implicit, &implicit));
        assert_eq!(
            begin(&implicit, None, &implicit).err(),
            Some("session has ended")
        );
        assert!(revive_session(&implicit));
        assert!(revive_session(&named));
    }

    #[test]
    fn revive_clears_the_tombstone_for_an_ended_id() {
        let sid = "test-revive-session-445566";
        use_session(sid).unwrap();
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
        use_session(&sid).unwrap();

        assert_eq!(revoke_sessions_with_prefix(prefix), 1);
        assert!(is_session_ended(&sid));
        assert!(use_session(&sid).is_err());
        assert!(
            !has_session_activity(&sid),
            "ordinary traffic must not resurrect a revoked runtime session"
        );

        assert!(revive_session(&sid));
        use_session(&sid).unwrap();
        assert!(has_session_activity(&sid));
        end_session(&sid);
        assert_eq!(forget_ended_sessions_with_prefix(prefix), 1);
        assert!(!is_session_ended(&sid));
    }

    #[test]
    fn a_terminal_end_during_idle_cleanup_blocks_revival_until_its_hooks_ran() {
        let sid = "__cua_runtime_idle-then-terminal:named";
        let owner = "idle-then-terminal-owner";
        activate_session(
            sid,
            Some("named"),
            owner,
            false,
            SessionTransport::Daemon,
            SessionClientKind::Cli,
        )
        .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let seen = reasons.clone();
        let _hook = register_scoped_session_end_hook(move |id| {
            if id != sid {
                return;
            }
            let reason = session_end_reason(id);
            seen.lock().unwrap().push(reason);
            if reason == Some(SessionEndReason::IdleTimeout) {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        });

        let idle = std::thread::spawn(|| evict_idle_with_prefix(Duration::ZERO, sid));
        entered_rx.recv().unwrap();
        end_session(sid);
        assert!(
            activate_or_revive_session_for_owner(
                sid,
                Some("named"),
                owner,
                false,
                SessionTransport::Daemon,
                SessionClientKind::Cli,
                None,
            )
            .is_err(),
            "revival must wait for the terminal end's hooks"
        );

        release_tx.send(()).unwrap();
        assert_eq!(idle.join().unwrap(), vec![sid.to_owned()]);
        assert_eq!(
            *reasons.lock().unwrap(),
            [
                Some(SessionEndReason::IdleTimeout),
                Some(SessionEndReason::Explicit)
            ]
        );
        assert_eq!(
            activate_or_revive_session_for_owner(
                sid,
                Some("named"),
                owner,
                false,
                SessionTransport::Daemon,
                SessionClientKind::Cli,
                None,
            ),
            Ok(true)
        );
        end_session(sid);
    }

    #[test]
    fn a_revival_during_a_slow_end_keeps_its_capture_policy() {
        let sid = "__cua_runtime_slow-end:named";
        let owner = "slow-end-owner";
        let start = || {
            activate_or_revive_session_for_owner(
                sid,
                Some("named"),
                owner,
                false,
                SessionTransport::Daemon,
                SessionClientKind::Cli,
                None,
            )
        };
        start().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let reader = register_scoped_cursor_outcome_reader(Arc::new(move |id| {
            if id == sid {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            bounded_cursor_outcome(false, false, false, None, false, 0)
        }));

        let end = std::thread::spawn(|| end_session(sid));
        entered_rx.recv().unwrap();
        // The end is waiting on a cursor reader; a revival in that window
        // runs the cleanup itself and then binds its new policy.
        assert_eq!(start(), Ok(true));
        crate::capture_scope::bind_session(sid, Some(CaptureScope::Desktop)).unwrap();
        release_tx.send(()).unwrap();
        end.join().unwrap();

        assert!(crate::capture_scope::get_session(sid).is_some());
        drop(reader);
        end_session(sid);
    }

    /// An end that found no call in flight and released the records lock
    /// still loses to a call admitted before it removes the record: the
    /// removal waits for that call instead of running cleanup under it.
    #[test]
    fn an_end_waits_for_a_call_admitted_after_it_decided() {
        let sid = "test-end-after-admit-SE5";
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let seen = reasons.clone();
        let _hook = register_scoped_session_end_hook(move |id| {
            if id == sid {
                seen.lock().unwrap().push(session_end_reason(id));
            }
        });
        use_session(sid).unwrap();
        let call = begin_session_dispatch(
            sid,
            None,
            sid,
            false,
            SessionTransport::Daemon,
            SessionClientKind::Direct,
        )
        .unwrap();
        // The end's second critical section, reached after its in-flight check.
        finish_session_end(sid, SessionEndReason::Explicit, true, None, None);
        assert!(
            !is_session_ended(sid),
            "the admitted call keeps its session"
        );
        assert!(
            reasons.lock().unwrap().is_empty(),
            "no cleanup under a live call"
        );
        drop(call);
        assert!(
            is_session_ended(sid),
            "the end completes when the call ends"
        );
        assert_eq!(*reasons.lock().unwrap(), [Some(SessionEndReason::Explicit)]);
    }

    #[test]
    fn an_end_session_deferred_behind_a_late_call_keeps_its_label_refused() {
        let label = "test-late-explicit-end-L4T3";
        let owner_a = "test-late-explicit-end-transport-a";
        let owner_b = "test-late-explicit-end-transport-b";
        let call = begin_session_dispatch(
            label,
            Some("label"),
            owner_a,
            false,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();
        // The end's second critical section, reached after its in-flight check.
        finish_session_end(label, SessionEndReason::Explicit, true, None, None);
        drop(call);
        assert!(is_session_ended(label));
        end_sessions_for_owner(owner_a, SessionEndReason::ProcessExit);
        assert!(
            !recreates_on_next_call(label, owner_b),
            "a label ended with end_session waits for a start_session"
        );
    }

    #[test]
    fn a_terminal_end_overrides_an_idle_end_deferred_behind_a_dispatch() {
        let sid = "__cua_runtime_deferred-idle:named";
        let owner = "deferred-idle-owner";
        let guard = begin_session_dispatch(
            sid,
            Some("named"),
            owner,
            false,
            SessionTransport::Daemon,
            SessionClientKind::Cli,
        )
        .unwrap();
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let seen = reasons.clone();
        let _hook = register_scoped_session_end_hook(move |id| {
            if id == sid {
                seen.lock().unwrap().push(session_end_reason(id));
            }
        });

        end_session_with_reason(sid, SessionEndReason::IdleTimeout, false, None);
        end_session(sid);
        drop(guard);

        assert_eq!(*reasons.lock().unwrap(), [Some(SessionEndReason::Explicit)]);
        assert!(is_session_ended(sid));
        assert!(revive_session_for_owner(sid, owner).is_ok());
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
        end_session_with_reason(idle, SessionEndReason::IdleTimeout, false, None);

        let revived = "test-observer-revived-MN34";
        use_session(revived).unwrap();
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

    /// cua-spacesd's `/mcp` ends a run by its public transport id, while the
    /// registry keyed the run's sessions (and cursors) by the runtime-private
    /// id. The end must reach those keys, including labelled sessions the
    /// run owns, and must not touch another run.
    #[test]
    fn trusted_adapter_transport_end_reaches_runtime_scoped_sessions() {
        let registry = crate::session_authorization::configured_registry().unwrap();
        let transport = "agent-test-trusted-end-7f3a";
        let other = "agent-test-trusted-end-other-7f3a";
        let scoped = registry.runtime_session_key(transport);
        let labelled = registry.runtime_session_key("test-trusted-end-label-7f3a");
        let other_scoped = registry.runtime_session_key(other);
        let ended = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = ended.clone();
        let _hook = register_scoped_session_end_hook(move |id| {
            seen.lock().unwrap().push(id.to_owned());
        });
        for (id, label, owner) in [
            (&scoped, None, &scoped),
            (&labelled, Some("test-trusted-end-label-7f3a"), &scoped),
            (&other_scoped, None, &other_scoped),
        ] {
            drop(
                begin_session_dispatch(
                    id,
                    label,
                    owner,
                    label.is_none(),
                    SessionTransport::McpHttp,
                    SessionClientKind::Mcp,
                )
                .unwrap(),
            );
        }

        assert_eq!(end_trusted_adapter_transport(transport), 2);
        let ended = ended.lock().unwrap().clone();
        assert!(ended.contains(&scoped), "{ended:?}");
        assert!(ended.contains(&labelled), "{ended:?}");
        assert!(!ended.contains(&other_scoped), "{ended:?}");
        assert!(is_session_ended(&scoped) && is_session_ended(&labelled));
        assert!(!is_session_ended(&other_scoped));
        assert!(!is_session_ended(transport), "the bare id is not a session");

        // A transport that never owned a session ends nothing.
        assert_eq!(
            end_trusted_adapter_transport("agent-test-never-seen-7f3a"),
            0
        );
        assert_eq!(end_trusted_adapter_transport("default"), 0);
        end_session(&other_scoped);
    }
}
