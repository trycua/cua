//! Session-scoped target / tab / page-ref capability store.
//!
//! Browser target ids (`bt-<uuid>`), tab ids (`tab-<uuid>`) and page refs
//! (`p<snapshot>:<index>`) are opaque, session-scoped capabilities:
//! they only resolve in the session that minted them, and the whole
//! namespace is dropped when that session ends. Ids are minted from a
//! process-global counter so they never collide across sessions — a
//! capability leaked into another session simply fails to resolve.
//!
//! Refs map internally to CDP `backendNodeId`s plus a [`FrameRef`]
//! recording which frame (main, same-process iframe, or OOPIF child
//! target) minted the node and that frame's document identity
//! (`frame_id` + `loader_id`) at snapshot time. Navigation invalidates
//! every snapshot of the navigated tab; stale refs refuse with
//! `browser_ref_stale`, and frame identity is re-proven against the
//! live frame tree before any mutation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::challenge::BrowserChallengeSource;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::semantic::SemanticDocument;
use super::types::{
    BindingQuality, EndpointAccessClass, EndpointTransport, ProcessFingerprint, Rect,
};

/// Browser action kinds proven for one semantic page ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserActionKind {
    Click,
    Type,
    Upload,
    Pointer,
    Scroll,
}

impl BrowserActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Type => "type",
            Self::Upload => "upload",
            Self::Pointer => "pointer",
            Self::Scroll => "scroll",
        }
    }
}

/// Browser-layout visibility. This is independent from native desktop
/// foreground or occlusion state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserVisibility {
    InViewport,
    NearViewport,
    Offscreen,
    CssHidden,
    NoLayout,
    PageOccluded,
    Unknown,
}

impl BrowserVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InViewport => "in_viewport",
            Self::NearViewport => "near_viewport",
            Self::Offscreen => "offscreen",
            Self::CssHidden => "css_hidden",
            Self::NoLayout => "no_layout",
            Self::PageOccluded => "page_occluded",
            Self::Unknown => "unknown",
        }
    }
}

/// Which frame kind a ref was minted in. Exposed on the wire as a
/// stable string via [`FrameKind::as_str`]; everything else about the
/// frame stays internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// The tab's main frame (including composed shadow DOM inside it).
    Main,
    /// A same-process iframe walked via `contentDocument`.
    Iframe,
    /// An out-of-process iframe reached through a capability-tested
    /// child session beneath the tab's target.
    Oopif,
}

impl FrameKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Iframe => "iframe",
            Self::Oopif => "oopif",
        }
    }
}

/// CDP frame/document identity captured at snapshot time. The
/// `loader_id` changes on every document load, so equality against the
/// live frame tree proves the ref's document is still the one that was
/// snapshotted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameIdentity {
    pub frame_id: String,
    pub loader_id: String,
}

/// The frame a ref belongs to, with everything needed to re-prove that
/// frame before mutation. Invariants enforced at mint time:
/// - `kind != Main` ⇒ `identity` is `Some` (unprovable frames are
///   omitted from snapshots, never guessed).
/// - `kind == Oopif` ⇔ `oopif_target_id` is `Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRef {
    pub kind: FrameKind,
    /// CDP target id of the OOPIF child target (contained beneath the
    /// bound tab), present only for `Oopif` refs.
    pub oopif_target_id: Option<String>,
    /// Document identity at snapshot time. `None` only on the
    /// v1-compat main-frame path where the endpoint cannot report a
    /// frame tree; node liveness checks remain the backstop there.
    pub identity: Option<FrameIdentity>,
}

impl FrameRef {
    /// The v1-compat main-frame ref: no frame tree available, identity
    /// unproven, mutation falls back to node-liveness checks only.
    pub fn main_unproven() -> Self {
        Self {
            kind: FrameKind::Main,
            oopif_target_id: None,
            identity: None,
        }
    }
}

/// One interactive element captured in a page snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct RefEntry {
    /// CDP backendNodeId — internal only, never exposed to callers.
    /// Valid in the tab's own session, or in the OOPIF child session
    /// named by `frame.oopif_target_id`.
    #[serde(skip_serializing)]
    pub backend_node_id: i64,
    pub node_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Semantic action kinds. Empty for legacy DOM refs, whose existing
    /// mutation behavior remains compatible during the v2 migration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<BrowserActionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<BrowserVisibility>,
    /// Semantic refs enforce their declared action set. Legacy DOM refs keep
    /// their existing permissive behavior during the versioned migration.
    #[serde(skip_serializing)]
    pub semantic: bool,
    /// Frame identity — internal; only the kind string is surfaced.
    #[serde(skip_serializing)]
    pub frame: FrameRef,
}

#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub id: u64,
    pub generation: u64,
    pub url: String,
    /// index → entry; the external ref is `p<id>:<index>`.
    pub refs: HashMap<u32, RefEntry>,
    pub(crate) semantic: Option<SemanticDocument>,
    pub(crate) semantic_root_identity: Option<FrameIdentity>,
    pub(crate) continuations: HashMap<String, SemanticContinuation>,
}

#[derive(Debug, Clone)]
pub struct SemanticScope {
    pub backend_node_id: i64,
    pub frame: FrameRef,
}

impl From<&RefEntry> for SemanticScope {
    fn from(entry: &RefEntry) -> Self {
        Self {
            backend_node_id: entry.backend_node_id,
            frame: entry.frame.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticContinuation {
    pub offset: usize,
    pub query: Option<String>,
    pub scope: Option<SemanticScope>,
    pub oopif_supported: bool,
    pub oopif_frames: usize,
}

#[derive(Debug, Clone)]
pub struct TabRecord {
    pub tab_id: String,
    pub cdp_target_id: String,
    pub title: String,
    pub url: String,
    /// Native-window selection proof captured at bind time. `None` means the
    /// selected tab could not be proven without activating or foregrounding a
    /// page, so the public `active` field must be JSON null.
    pub active: Option<bool>,
    pub generation: u64,
    /// Set when navigation may have dispatched but its outcome was not
    /// committed. Mutations stay paused until exact explicit resume.
    pub(crate) navigation_blocker_id: Option<String>,
    pub snapshots: HashMap<u64, SnapshotRecord>,
}

/// One bound browser target: the full evidence set captured at bind
/// time, revalidated before every mutation.
#[derive(Debug, Clone)]
pub struct TargetRecord {
    pub target_id: String,
    pub pid: i64,
    pub window_id: u64,
    pub ws_url: String,
    pub endpoint_owner_pid: i64,
    pub endpoint_transport: EndpointTransport,
    pub endpoint_access_class: EndpointAccessClass,
    /// CDP connection generation that minted this capability. Zero denotes
    /// the legacy/non-grant route.
    pub generation: u64,
    /// Internal transport owner that proved an existing-profile grant or a
    /// driver-owned browser lifecycle. The public session remains the target
    /// namespace and is checked independently.
    pub transport_session: Option<String>,
    pub fingerprint: ProcessFingerprint,
    pub native_title: String,
    pub native_bounds: Rect,
    pub cdp_target_id: String,
    /// CDP browser window id, or None for the exact single-page embedded route.
    pub cdp_window_id: Option<i64>,
    pub quality: BindingQuality,
    pub tabs: HashMap<String, TabRecord>,
}

#[derive(Default)]
struct SessionTargets {
    targets: HashMap<String, TargetRecord>,
    origin_blockers: HashMap<String, OriginBlocker>,
    blocker_capacity_id: Option<String>,
}

const MAX_BLOCKED_ORIGINS_PER_SESSION: usize = 64;
const RATE_LIMIT_BACKOFF_BASE: Duration = Duration::from_secs(2);
const RATE_LIMIT_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const RATE_LIMIT_SERVER_RETRY_MAX: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginBlockerKind {
    AntiBotChallenge,
    RateLimited,
    NavigationOutcomeUnknown,
    SafetyCapacity,
}

#[derive(Debug, Clone)]
pub(crate) struct OriginBlocker {
    blocker_id: String,
    origin: String,
    kind: OriginBlockerKind,
    challenge_document: Option<FrameIdentity>,
    detection_source: &'static str,
    retry_at: Option<Instant>,
    attempt: u32,
    server_directed: bool,
    server_retry_after_capped: bool,
}

impl OriginBlocker {
    pub(crate) fn blocker_id(&self) -> &str {
        &self.blocker_id
    }

    pub(crate) fn requires_challenge_document_reproof(&self) -> bool {
        self.kind == OriginBlockerKind::AntiBotChallenge
    }

    fn active(&self, now: Instant) -> bool {
        match self.kind {
            OriginBlockerKind::AntiBotChallenge
            | OriginBlockerKind::NavigationOutcomeUnknown
            | OriginBlockerKind::SafetyCapacity => true,
            OriginBlockerKind::RateLimited => {
                self.retry_at.map(|retry_at| now < retry_at).unwrap_or(true)
            }
        }
    }

    pub(crate) fn to_value(&self, now: Instant) -> Value {
        let retry_after_ms = self.retry_at.map(|retry_at| {
            let milliseconds = retry_at.saturating_duration_since(now).as_millis();
            u64::try_from(milliseconds).unwrap_or(u64::MAX)
        });
        let (kind, requires_user, handling) = match self.kind {
            OriginBlockerKind::AntiBotChallenge if self.origin.is_empty() => {
                ("anti_bot_challenge", true, "navigate_away_or_end_session")
            }
            OriginBlockerKind::AntiBotChallenge => (
                "anti_bot_challenge",
                true,
                "explicit_resume_or_user_handoff",
            ),
            OriginBlockerKind::RateLimited => {
                ("rate_limited", false, "wait_until_retry_or_explicit_resume")
            }
            OriginBlockerKind::NavigationOutcomeUnknown if self.origin.is_empty() => (
                "navigation_outcome_unknown",
                true,
                "refresh_state_then_explicit_resume_or_end_session",
            ),
            OriginBlockerKind::NavigationOutcomeUnknown => (
                "navigation_outcome_unknown",
                true,
                "explicit_resume_or_end_session",
            ),
            OriginBlockerKind::SafetyCapacity => (
                "safety_capacity",
                true,
                "end_session_before_more_browser_actions",
            ),
        };
        json!({
            "required": true,
            "blocker_id": self.blocker_id,
            "kind": kind,
            "origin": if self.origin.is_empty() {
                Value::Null
            } else {
                Value::String(self.origin.clone())
            },
            "detection_source": self.detection_source,
            "retry_after_ms": retry_after_ms,
            "requires_user": requires_user,
            "handling": handling,
            "attempt": self.attempt,
            "server_directed_retry": self.server_directed,
            "server_retry_after_capped": self.server_retry_after_capped,
        })
    }

    pub(crate) fn requires_session_end(&self) -> bool {
        self.kind == OriginBlockerKind::SafetyCapacity
    }
}

#[derive(Debug)]
pub(crate) enum ClearOriginBlockerFailure {
    Missing,
    Mismatch(OriginBlocker),
    SessionEndRequired(OriginBlocker),
}

fn new_blocker_id() -> String {
    format!("blocker-{}", Uuid::new_v4())
}

fn safety_capacity_blocker(origin: &str, blocker_id: &str) -> OriginBlocker {
    OriginBlocker {
        blocker_id: blocker_id.to_owned(),
        origin: origin.to_owned(),
        kind: OriginBlockerKind::SafetyCapacity,
        challenge_document: None,
        detection_source: "blocker_capacity",
        retry_at: None,
        attempt: 1,
        server_directed: false,
        server_retry_after_capped: false,
    }
}

fn navigation_outcome_blocker(origin: &str, blocker_id: &str) -> OriginBlocker {
    OriginBlocker {
        blocker_id: blocker_id.to_owned(),
        origin: origin.to_owned(),
        kind: OriginBlockerKind::NavigationOutcomeUnknown,
        challenge_document: None,
        detection_source: "navigation_dispatch",
        retry_at: None,
        attempt: 1,
        server_directed: false,
        server_retry_after_capped: false,
    }
}

fn install_challenge_blocker(
    session_targets: &mut SessionTargets,
    origin: &str,
    source: BrowserChallengeSource,
    document_identity: Option<&FrameIdentity>,
) -> OriginBlocker {
    if let Some(blocker_id) = session_targets.blocker_capacity_id.as_deref() {
        return safety_capacity_blocker(origin, blocker_id);
    }
    if let Some(challenge) = session_targets
        .origin_blockers
        .get(origin)
        .filter(|blocker| {
            blocker.kind == OriginBlockerKind::AntiBotChallenge
                && document_identity.is_some()
                && blocker.challenge_document.as_ref() == document_identity
        })
    {
        return challenge.clone();
    }
    let blocker = OriginBlocker {
        blocker_id: new_blocker_id(),
        origin: origin.to_owned(),
        kind: OriginBlockerKind::AntiBotChallenge,
        challenge_document: document_identity.cloned(),
        detection_source: source.as_str(),
        retry_at: None,
        attempt: 1,
        server_directed: false,
        server_retry_after_capped: false,
    };
    match insert_bounded_origin_blocker(session_targets, origin, blocker.clone()) {
        Ok(()) => blocker,
        Err(capacity_id) => safety_capacity_blocker(origin, &capacity_id),
    }
}

fn install_rate_limit_blocker(
    session_targets: &mut SessionTargets,
    origin: &str,
    server_retry_after: Option<Duration>,
    now: Instant,
) -> OriginBlocker {
    if let Some(blocker_id) = session_targets.blocker_capacity_id.as_deref() {
        return safety_capacity_blocker(origin, blocker_id);
    }
    if let Some(challenge) = session_targets
        .origin_blockers
        .get(origin)
        .filter(|blocker| blocker.kind == OriginBlockerKind::AntiBotChallenge)
    {
        return challenge.clone();
    }
    let existing_rate_limit = session_targets
        .origin_blockers
        .get(origin)
        .filter(|blocker| blocker.kind == OriginBlockerKind::RateLimited);
    let attempt = existing_rate_limit
        .map(|blocker| blocker.attempt.saturating_add(1))
        .unwrap_or(1);
    let (delay, server_retry_after_capped) = server_retry_after.map_or_else(
        || (bounded_rate_limit_backoff(attempt), false),
        |delay| {
            (
                delay.min(RATE_LIMIT_SERVER_RETRY_MAX),
                delay > RATE_LIMIT_SERVER_RETRY_MAX,
            )
        },
    );
    let blocker = OriginBlocker {
        // Every new 429 is newer server evidence. Rotate the capability so a
        // resume decision for an earlier retry window cannot clear it.
        blocker_id: new_blocker_id(),
        origin: origin.to_owned(),
        kind: OriginBlockerKind::RateLimited,
        challenge_document: None,
        detection_source: "http_status",
        // A hostile or malformed Retry-After must not turn an arithmetic
        // overflow into an origin pause with no end time.
        retry_at: Some(
            now.checked_add(delay)
                .or_else(|| now.checked_add(RATE_LIMIT_BACKOFF_MAX))
                .unwrap_or(now),
        ),
        attempt,
        server_directed: server_retry_after.is_some(),
        server_retry_after_capped,
    };
    match insert_bounded_origin_blocker(session_targets, origin, blocker.clone()) {
        Ok(()) => blocker,
        Err(capacity_id) => safety_capacity_blocker(origin, &capacity_id),
    }
}

fn bounded_rate_limit_backoff(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    let base_ms = RATE_LIMIT_BACKOFF_BASE
        .as_millis()
        .saturating_mul(1_u128 << exponent)
        .min(RATE_LIMIT_BACKOFF_MAX.as_millis());
    let mut random = [0_u8; 8];
    let _ = getrandom::fill(&mut random);
    let jitter_ceiling = (base_ms / 4).max(1);
    let jitter_ms = u128::from(u64::from_le_bytes(random)) % jitter_ceiling;
    Duration::from_millis(
        u64::try_from((base_ms + jitter_ms).min(RATE_LIMIT_BACKOFF_MAX.as_millis()))
            .unwrap_or(u64::MAX),
    )
}

/// Parse an external page ref of the form `p<snapshot>:<index>`.
/// Anything else — including refs from other namespaces such as the
/// accessibility `element_index` / element-token space — is rejected.
pub fn parse_ref(external: &str) -> Option<(u64, u32)> {
    let rest = external.strip_prefix('p')?;
    let (snap, idx) = rest.split_once(':')?;
    // Reject leading '+', whitespace, empty parts: only plain digits.
    if snap.is_empty()
        || idx.is_empty()
        || !snap.bytes().all(|b| b.is_ascii_digit())
        || !idx.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((snap.parse().ok()?, idx.parse().ok()?))
}

/// Format the external ref for a snapshot/index pair.
pub fn format_ref(snapshot_id: u64, index: u32) -> String {
    format!("p{snapshot_id}:{index}")
}

pub struct BrowserStore {
    inner: Mutex<HashMap<String, SessionTargets>>,
}

impl BrowserStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn next(&self) -> u64 {
        static NEXT_BROWSER_SNAPSHOT_ID: AtomicU64 = AtomicU64::new(1);
        NEXT_BROWSER_SNAPSHOT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Mint a target id and insert the record under `session`.
    /// Panics never; the caller has already enforced that `session` is
    /// an explicit (non-default) session.
    pub fn mint_target(&self, session: &str, mut record: TargetRecord) -> String {
        let id = format!("bt-{}", Uuid::new_v4());
        record.target_id = id.clone();
        self.inner
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_default()
            .targets
            .insert(id.clone(), record);
        id
    }

    /// Mint a fresh tab id (caller stores it via [`Self::update_target`]).
    pub fn mint_tab_id(&self) -> String {
        format!("tab-{}", Uuid::new_v4())
    }

    /// Mint a fresh snapshot id.
    pub fn mint_snapshot_id(&self) -> u64 {
        self.next()
    }

    /// Look up a target capability. Unknown ids — including ids minted
    /// by a *different* session — refuse with `browser_binding_stale`:
    /// the capability is simply not valid here.
    pub fn get_target(
        &self,
        session: &str,
        target_id: &str,
    ) -> Result<TargetRecord, BrowserRefusal> {
        self.inner
            .lock()
            .unwrap()
            .get(session)
            .and_then(|s| s.targets.get(target_id))
            .cloned()
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserBindingStale,
                    format!(
                        "target {target_id} is not a live binding in this session — \
                         re-run get_browser_state with pid + window_id"
                    ),
                )
            })
    }

    /// Mutate a stored target in place. No-op if it disappeared.
    pub fn update_target(&self, session: &str, target_id: &str, f: impl FnOnce(&mut TargetRecord)) {
        if let Some(rec) = self
            .inner
            .lock()
            .unwrap()
            .get_mut(session)
            .and_then(|s| s.targets.get_mut(target_id))
        {
            f(rec);
        }
    }

    /// Resolve an external page ref to a backendNodeId within one tab's
    /// live snapshot namespace.
    pub fn resolve_ref(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        external: &str,
    ) -> Result<RefEntry, BrowserRefusal> {
        let (snap, idx) = parse_ref(external).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRefStale,
                format!(
                    "ref {external:?} is not a browser page ref — expected the \
                     p<snapshot>:<index> namespace from get_browser_state"
                ),
            )
        })?;
        let target = self.get_target(session, target_id)?;
        let tab = target.tabs.get(tab_id).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        tab.snapshots
            .get(&snap)
            .filter(|snapshot| snapshot.generation == target.generation)
            .and_then(|snapshot| snapshot.refs.get(&idx))
            .cloned()
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    format!(
                        "ref {external} is stale — the page navigated or the snapshot \
                         was superseded; re-run get_browser_state to re-snapshot"
                    ),
                )
            })
    }

    /// Resolve an opaque continuation within the same session, target, tab,
    /// snapshot generation, and currently-live snapshot namespace.
    pub fn resolve_semantic_continuation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        token: &str,
    ) -> Result<(SnapshotRecord, SemanticContinuation), BrowserRefusal> {
        let target = self.get_target(session, target_id)?;
        let tab = target.tabs.get(tab_id).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        tab.snapshots
            .values()
            .find(|snapshot| {
                snapshot.generation == target.generation
                    && snapshot.semantic.is_some()
                    && snapshot.continuations.contains_key(token)
            })
            .and_then(|snapshot| {
                snapshot
                    .continuations
                    .get(token)
                    .cloned()
                    .map(|continuation| (snapshot.clone(), continuation))
            })
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    "the semantic continuation is stale or does not belong to this session and tab",
                )
            })
    }

    /// Publish a completed semantic snapshot, refusing rather than reporting
    /// success if its target, tab, or connection generation changed while CDP
    /// collection was in flight.
    pub(crate) fn publish_semantic_snapshot(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        snapshot: SnapshotRecord,
    ) -> Result<(), BrowserRefusal> {
        let mut inner = self.inner.lock().unwrap();
        let target = inner
            .get_mut(session)
            .and_then(|session| session.targets.get_mut(target_id))
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserBindingStale,
                    format!(
                        "target {target_id} is not a live binding in this session — \
                         re-run get_browser_state with pid + window_id"
                    ),
                )
            })?;
        if target.generation != snapshot.generation {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingStale,
                "the browser connection changed while the semantic snapshot was being collected",
            ));
        }
        let tab = target.tabs.get_mut(tab_id).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        tab.snapshots.clear();
        tab.snapshots.insert(snapshot.id, snapshot);
        Ok(())
    }

    /// Commit one continuation page and consume its token in the same store
    /// critical section. A racing caller can do collection work, but cannot
    /// consume the same capability or publish a second result.
    pub(crate) fn commit_semantic_continuation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        snapshot_id: u64,
        token: &str,
        refs: HashMap<u32, RefEntry>,
        next: Option<(String, SemanticContinuation)>,
    ) -> Result<(), BrowserRefusal> {
        let mut inner = self.inner.lock().unwrap();
        let target = inner
            .get_mut(session)
            .and_then(|session| session.targets.get_mut(target_id))
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserBindingStale,
                    format!(
                        "target {target_id} is not a live binding in this session — \
                         re-run get_browser_state with pid + window_id"
                    ),
                )
            })?;
        let generation = target.generation;
        let tab = target.tabs.get_mut(tab_id).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        let snapshot = tab
            .snapshots
            .get_mut(&snapshot_id)
            .filter(|snapshot| {
                snapshot.generation == generation
                    && snapshot.semantic.is_some()
                    && snapshot.continuations.contains_key(token)
            })
            .ok_or_else(|| {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    "the semantic continuation is stale or does not belong to this session and tab",
                )
            })?;
        snapshot.continuations.remove(token);
        snapshot.refs.extend(refs);
        if let Some((token, continuation)) = next {
            snapshot.continuations.insert(token, continuation);
        }
        Ok(())
    }

    /// Drop every snapshot of one tab (navigation invalidates refs).
    pub fn invalidate_tab_snapshots(&self, session: &str, target_id: &str, tab_id: &str) {
        self.update_target(session, target_id, |rec| {
            if let Some(tab) = rec.tabs.get_mut(tab_id) {
                tab.snapshots.clear();
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn block_origin_for_challenge(
        &self,
        session: &str,
        origin: &str,
        source: BrowserChallengeSource,
        document_identity: Option<&FrameIdentity>,
    ) -> OriginBlocker {
        // Production callers own the session transition guard: insertion can
        // cross the bounded-store limit and latch the whole session closed.
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.entry(session.to_owned()).or_default();
        install_challenge_blocker(session_targets, origin, source, document_identity)
    }

    /// Install a challenge only while the exact snapshot target generation is
    /// still live. Cancellation-owned commits use this check so session end or
    /// later session-id reuse cannot resurrect stale blocker state.
    pub(crate) fn block_origin_for_challenge_if_target_matches(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        target_generation: u64,
        origin: &str,
        source: BrowserChallengeSource,
        document_identity: Option<&FrameIdentity>,
    ) -> Option<OriginBlocker> {
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.get_mut(session)?;
        let target_matches = session_targets
            .targets
            .get(target_id)
            .is_some_and(|target| {
                target.generation == target_generation && target.tabs.contains_key(tab_id)
            });
        target_matches
            .then(|| install_challenge_blocker(session_targets, origin, source, document_identity))
    }

    /// Persist an exact-tab pause when navigation may have dispatched but its
    /// final outcome was not committed. Repeated uncertain outcomes preserve
    /// the same capability until the caller explicitly clears it.
    pub(crate) fn block_tab_for_unknown_navigation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        observed_origin: &str,
    ) -> Option<OriginBlocker> {
        let mut inner = self.inner.lock().unwrap();
        let tab = inner
            .get_mut(session)?
            .targets
            .get_mut(target_id)?
            .tabs
            .get_mut(tab_id)?;
        let blocker_id = tab
            .navigation_blocker_id
            .get_or_insert_with(new_blocker_id)
            .clone();
        Some(navigation_outcome_blocker(observed_origin, &blocker_id))
    }

    pub(crate) fn active_tab_navigation_blocker(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        live_origin: &str,
    ) -> Option<OriginBlocker> {
        let inner = self.inner.lock().unwrap();
        let blocker_id = inner
            .get(session)?
            .targets
            .get(target_id)?
            .tabs
            .get(tab_id)?
            .navigation_blocker_id
            .as_deref()?;
        Some(navigation_outcome_blocker(live_origin, blocker_id))
    }

    pub(crate) fn clear_tab_navigation_blocker_if_matches(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        live_origin: &str,
        blocker_id: &str,
    ) -> Result<OriginBlocker, ClearOriginBlockerFailure> {
        let mut inner = self.inner.lock().unwrap();
        let Some(tab) = inner
            .get_mut(session)
            .and_then(|targets| targets.targets.get_mut(target_id))
            .and_then(|target| target.tabs.get_mut(tab_id))
        else {
            return Err(ClearOriginBlockerFailure::Missing);
        };
        let Some(current_id) = tab.navigation_blocker_id.as_deref() else {
            return Err(ClearOriginBlockerFailure::Missing);
        };
        let current = navigation_outcome_blocker(live_origin, current_id);
        if current_id != blocker_id {
            return Err(ClearOriginBlockerFailure::Mismatch(current));
        }
        tab.navigation_blocker_id = None;
        Ok(current)
    }

    #[cfg(test)]
    pub(crate) fn block_origin_for_rate_limit(
        &self,
        session: &str,
        origin: &str,
        server_retry_after: Option<Duration>,
    ) -> OriginBlocker {
        // Production callers own the session transition guard for the same
        // reason as challenge insertion: capacity failure is session-wide.
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.entry(session.to_owned()).or_default();
        install_rate_limit_blocker(session_targets, origin, server_retry_after, now)
    }

    /// Commit a proven main-document response only if the exact navigation
    /// target generation is still live. The outer transition guard orders this
    /// store update against admitted browser actions.
    pub(crate) fn apply_navigation_response_if_target_matches(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        target_generation: u64,
        origin: &str,
        status: u16,
        retry_after: Option<Duration>,
    ) -> Option<Option<OriginBlocker>> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.get_mut(session)?;
        let target_matches = session_targets
            .targets
            .get(target_id)
            .is_some_and(|target| {
                target.generation == target_generation && target.tabs.contains_key(tab_id)
            });
        if !target_matches {
            return None;
        }
        if status == 429 {
            return Some(Some(install_rate_limit_blocker(
                session_targets,
                origin,
                retry_after,
                now,
            )));
        }
        if let Some(blocker_id) = session_targets.blocker_capacity_id.as_deref() {
            return Some(Some(safety_capacity_blocker(origin, blocker_id)));
        }
        if let Some(blocker) = session_targets
            .origin_blockers
            .get(origin)
            .filter(|blocker| blocker.active(now))
            .cloned()
        {
            return Some(Some(blocker));
        }
        if (200..400).contains(&status)
            && session_targets
                .origin_blockers
                .get(origin)
                .is_some_and(|blocker| {
                    blocker.kind == OriginBlockerKind::RateLimited && !blocker.active(now)
                })
        {
            session_targets.origin_blockers.remove(origin);
        }
        Some(None)
    }

    pub(crate) fn active_origin_blocker(
        &self,
        session: &str,
        origin: &str,
    ) -> Option<OriginBlocker> {
        let now = Instant::now();
        self.inner.lock().unwrap().get(session).and_then(|targets| {
            if let Some(blocker_id) = targets.blocker_capacity_id.as_deref() {
                Some(safety_capacity_blocker(origin, blocker_id))
            } else {
                targets
                    .origin_blockers
                    .get(origin)
                    .filter(|blocker| blocker.active(now))
                    .cloned()
            }
        })
    }

    pub(crate) fn clear_origin_blocker_if_matches(
        &self,
        session: &str,
        origin: &str,
        blocker_id: &str,
        live_challenge_document: Option<&FrameIdentity>,
    ) -> Result<OriginBlocker, ClearOriginBlockerFailure> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let Some(targets) = inner.get_mut(session) else {
            return Err(ClearOriginBlockerFailure::Missing);
        };
        if let Some(capacity_id) = targets.blocker_capacity_id.as_deref() {
            return Err(ClearOriginBlockerFailure::SessionEndRequired(
                safety_capacity_blocker(origin, capacity_id),
            ));
        }
        let Some(current) = targets
            .origin_blockers
            .get(origin)
            .filter(|blocker| blocker.active(now))
            .cloned()
        else {
            return Err(ClearOriginBlockerFailure::Missing);
        };
        if current.blocker_id != blocker_id {
            return Err(ClearOriginBlockerFailure::Mismatch(current));
        }
        if current.kind == OriginBlockerKind::AntiBotChallenge
            && (current.challenge_document.is_none()
                || current.challenge_document.as_ref() != live_challenge_document)
        {
            // The explicit decision is a capability for one proven challenge
            // document. A reload, same-origin replacement, or unavailable
            // loader proof creates a new incident rather than letting the old
            // decision clear whatever is currently live.
            let mut replacement = current;
            replacement.blocker_id = new_blocker_id();
            replacement.challenge_document = live_challenge_document.cloned();
            targets
                .origin_blockers
                .insert(origin.to_owned(), replacement.clone());
            return Err(ClearOriginBlockerFailure::Mismatch(replacement));
        }
        targets.origin_blockers.remove(origin);
        Ok(current)
    }

    /// Forget an elapsed rate-limit incident after a successful document
    /// response. Active pauses and challenge blockers are never cleared here.
    #[cfg(test)]
    pub(crate) fn clear_elapsed_rate_limit_after_success(
        &self,
        session: &str,
        origin: &str,
    ) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let Some(blockers) = inner
            .get_mut(session)
            .map(|targets| &mut targets.origin_blockers)
        else {
            return false;
        };
        let elapsed = blockers.get(origin).is_some_and(|blocker| {
            blocker.kind == OriginBlockerKind::RateLimited && !blocker.active(now)
        });
        elapsed && blockers.remove(origin).is_some()
    }

    /// Drop the whole namespace for an ended session. Wired to
    /// `session::register_session_end_hook` by the engine.
    pub fn remove_session(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
    }

    /// Invalidate every capability minted for one browser endpoint before a
    /// reconnect generation becomes visible.
    pub fn invalidate_endpoint_generation(&self, pid: i64, generation: u64) -> usize {
        let mut removed = 0;
        for session in self.inner.lock().unwrap().values_mut() {
            let before = session.targets.len();
            session
                .targets
                .retain(|_, target| !(target.pid == pid && target.generation == generation));
            removed += before - session.targets.len();
        }
        removed
    }

    /// Number of live targets in a session (diagnostics/tests).
    pub fn target_count(&self, session: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .get(session)
            .map_or(0, |s| s.targets.len())
    }
}

fn insert_bounded_origin_blocker(
    session: &mut SessionTargets,
    origin: &str,
    blocker: OriginBlocker,
) -> Result<(), String> {
    if let Some(blocker_id) = session.blocker_capacity_id.as_ref() {
        return Err(blocker_id.clone());
    }
    if !session.origin_blockers.contains_key(origin)
        && session.origin_blockers.len() >= MAX_BLOCKED_ORIGINS_PER_SESSION
    {
        // Keep elapsed incidents long enough to increase repeated-429 backoff;
        // discard them only when the bounded map needs room.
        let now = Instant::now();
        session
            .origin_blockers
            .retain(|_, existing| existing.active(now));
    }
    if !session.origin_blockers.contains_key(origin)
        && session.origin_blockers.len() >= MAX_BLOCKED_ORIGINS_PER_SESSION
    {
        // Every retained entry is still active. Keep those exact blockers and
        // fail the whole session closed rather than silently making either an
        // old or newly observed origin actionable. Session end is the only
        // safe reset because the omitted origin set is no longer enumerable.
        let blocker_id = new_blocker_id();
        session.blocker_capacity_id = Some(blocker_id.clone());
        return Err(blocker_id);
    }
    session.origin_blockers.insert(origin.to_owned(), blocker);
    Ok(())
}

impl Default for BrowserStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::BindingQuality;

    fn record() -> TargetRecord {
        TargetRecord {
            target_id: String::new(),
            pid: 42,
            window_id: 7,
            ws_url: "ws://127.0.0.1:9222/devtools/browser/x".into(),
            endpoint_owner_pid: 42,
            endpoint_transport: EndpointTransport::LegacyJsonVersion,
            endpoint_access_class: EndpointAccessClass::EmbeddedApplication,
            generation: 0,
            transport_session: None,
            fingerprint: ProcessFingerprint {
                pid: 42,
                start_time: Some(1),
                executable: None,
            },
            native_title: "Docs - Chrome".into(),
            native_bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
            cdp_target_id: "CDP1".into(),
            cdp_window_id: Some(11),
            quality: BindingQuality::Exact,
            tabs: HashMap::new(),
        }
    }

    fn document_identity(loader_id: &str) -> FrameIdentity {
        FrameIdentity {
            frame_id: "frame-main".to_owned(),
            loader_id: loader_id.to_owned(),
        }
    }

    fn store_with_ref() -> (BrowserStore, String, String, String) {
        let store = BrowserStore::new();
        let tid = store.mint_target("sess-a", record());
        let tab_id = store.mint_tab_id();
        let snap_id = store.mint_snapshot_id();
        let ext = format_ref(snap_id, 0);
        store.update_target("sess-a", &tid, |rec| {
            let mut refs = HashMap::new();
            refs.insert(
                0,
                RefEntry {
                    backend_node_id: 555,
                    node_name: "button".into(),
                    label: Some("Submit".into()),
                    actions: Vec::new(),
                    visibility: None,
                    semantic: false,
                    frame: FrameRef {
                        kind: FrameKind::Main,
                        oopif_target_id: None,
                        identity: Some(FrameIdentity {
                            frame_id: "F_MAIN".into(),
                            loader_id: "L1".into(),
                        }),
                    },
                },
            );
            rec.tabs.insert(
                tab_id.clone(),
                TabRecord {
                    tab_id: tab_id.clone(),
                    cdp_target_id: "CDP1".into(),
                    title: "Example".into(),
                    url: "https://example.test".into(),
                    active: Some(true),
                    generation: 0,
                    navigation_blocker_id: None,
                    snapshots: HashMap::from([(
                        snap_id,
                        SnapshotRecord {
                            id: snap_id,
                            generation: 0,
                            url: "https://example.test".into(),
                            refs,
                            semantic: None,
                            semantic_root_identity: None,
                            continuations: HashMap::new(),
                        },
                    )]),
                },
            );
        });
        (store, tid, tab_id, ext)
    }

    #[test]
    fn ref_parsing_accepts_only_the_p_namespace() {
        assert_eq!(parse_ref("p12:5"), Some((12, 5)));
        assert_eq!(parse_ref("p0:0"), Some((0, 0)));
        for bad in [
            "e12", "12:5", "p:5", "p12:", "p-1:2", "p 1:2", "p1:+2", "p1", "",
        ] {
            assert_eq!(parse_ref(bad), None, "must reject {bad:?}");
        }
    }

    #[test]
    fn resolve_ref_happy_path() {
        let (store, tid, tab, ext) = store_with_ref();
        let entry = store.resolve_ref("sess-a", &tid, &tab, &ext).unwrap();
        assert_eq!(entry.backend_node_id, 555);
        assert_eq!(entry.node_name, "button");
        assert_eq!(entry.frame.kind, FrameKind::Main);
        assert_eq!(
            entry.frame.identity.as_ref().map(|i| i.loader_id.as_str()),
            Some("L1")
        );
    }

    #[test]
    fn ref_entry_serialization_hides_internal_identifiers() {
        let (store, tid, tab, ext) = store_with_ref();
        let entry = store.resolve_ref("sess-a", &tid, &tab, &ext).unwrap();
        let v = serde_json::to_value(&entry).unwrap();
        assert!(v.get("backend_node_id").is_none(), "{v}");
        assert!(v.get("frame").is_none(), "frame identity is internal: {v}");
    }

    #[test]
    fn target_ids_do_not_resolve_in_a_foreign_session() {
        let (store, tid, tab, ext) = store_with_ref();
        let err = store.resolve_ref("sess-b", &tid, &tab, &ext).unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserBindingStale);
        let err = store.get_target("sess-b", &tid).unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserBindingStale);
    }

    #[test]
    fn page_refs_do_not_alias_across_runtime_owned_stores() {
        let (_store_a, _target_a, _tab_a, ref_a) = store_with_ref();
        let (store_b, target_b, tab_b, ref_b) = store_with_ref();
        assert_ne!(ref_a, ref_b);
        let error = store_b
            .resolve_ref("sess-a", &target_b, &tab_b, &ref_a)
            .unwrap_err();
        assert_eq!(error.code, BrowserRefusalCode::BrowserRefStale);
    }

    #[test]
    fn foreign_namespace_refs_are_refused_as_stale() {
        let (store, tid, tab, _) = store_with_ref();
        // An accessibility element_index-style ref must not resolve.
        let err = store.resolve_ref("sess-a", &tid, &tab, "e42").unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserRefStale);
    }

    #[test]
    fn unknown_snapshot_or_index_is_stale() {
        let (store, tid, tab, _) = store_with_ref();
        let err = store
            .resolve_ref("sess-a", &tid, &tab, "p999999:0")
            .unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserRefStale);
    }

    #[test]
    fn unknown_tab_is_tab_not_found() {
        let (store, tid, _, ext) = store_with_ref();
        let err = store
            .resolve_ref("sess-a", &tid, "tab999", &ext)
            .unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserTabNotFound);
    }

    #[test]
    fn navigation_invalidates_tab_refs() {
        let (store, tid, tab, ext) = store_with_ref();
        assert!(store.resolve_ref("sess-a", &tid, &tab, &ext).is_ok());
        store.invalidate_tab_snapshots("sess-a", &tid, &tab);
        let err = store.resolve_ref("sess-a", &tid, &tab, &ext).unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserRefStale);
    }

    #[test]
    fn remove_session_drops_the_whole_namespace() {
        let (store, tid, tab, ext) = store_with_ref();
        assert_eq!(store.target_count("sess-a"), 1);
        store.remove_session("sess-a");
        assert_eq!(store.target_count("sess-a"), 0);
        let err = store.resolve_ref("sess-a", &tid, &tab, &ext).unwrap_err();
        assert_eq!(err.code, BrowserRefusalCode::BrowserBindingStale);
    }

    #[test]
    fn generation_invalidation_is_browser_wide_and_exact() {
        let store = BrowserStore::new();
        let mut old = record();
        old.generation = 1;
        let mut current = record();
        current.generation = 2;
        let mut other_process = record();
        other_process.pid = 99;
        other_process.endpoint_owner_pid = 99;
        other_process.fingerprint.pid = 99;
        other_process.generation = 1;
        store.mint_target("session-a", old);
        store.mint_target("session-b", current);
        store.mint_target("session-c", other_process);

        assert_eq!(store.invalidate_endpoint_generation(42, 1), 1);
        assert_eq!(store.target_count("session-a"), 0);
        assert_eq!(store.target_count("session-b"), 1);
        assert_eq!(store.target_count("session-c"), 1);
    }

    #[test]
    fn minted_ids_are_unique_across_sessions() {
        let store = BrowserStore::new();
        let a = store.mint_target("s1", record());
        let b = store.mint_target("s2", record());
        assert_ne!(a, b, "capability ids must never collide across sessions");
    }

    #[test]
    fn challenge_blockers_are_origin_local_session_state() {
        let store = BrowserStore::new();
        let identity = document_identity("loader-1");
        let blocker = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Semantic,
            Some(&identity),
        );
        let blocker_id = blocker.to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            blocker.to_value(Instant::now())["detection_source"],
            "semantic"
        );
        let repeated = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Semantic,
            Some(&identity),
        );
        assert_eq!(
            repeated.to_value(Instant::now())["blocker_id"],
            blocker_id,
            "re-observing the same active challenge must preserve its capability"
        );

        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_some());
        assert!(store
            .active_origin_blocker("session-a", "https://other.example")
            .is_none());
        assert!(store
            .active_origin_blocker("session-b", "https://blocked.example")
            .is_none());
        assert!(store
            .clear_origin_blocker_if_matches(
                "session-a",
                "https://blocked.example",
                &blocker_id,
                Some(&identity),
            )
            .is_ok());
        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_none());
        let replacement = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Semantic,
            Some(&identity),
        );
        assert_ne!(
            replacement.to_value(Instant::now())["blocker_id"],
            blocker_id,
            "a cleared challenge followed by a new observation is a new incident"
        );
    }

    #[test]
    fn challenge_blocker_rotates_for_a_new_or_unproven_document() {
        let store = BrowserStore::new();
        let first = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Url,
            Some(&document_identity("loader-1")),
        );
        let first_id = first.to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(first.to_value(Instant::now())["detection_source"], "url");

        let reloaded = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Url,
            Some(&document_identity("loader-2")),
        );
        let reloaded_id = reloaded.to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(reloaded_id, first_id);

        let unproven = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Url,
            None,
        );
        assert_ne!(
            unproven.to_value(Instant::now())["blocker_id"],
            reloaded_id,
            "identity equality must be proven before preserving a blocker capability"
        );
    }

    #[test]
    fn unknown_navigation_blocker_is_tab_local_and_exactly_cleared() {
        let (store, target_id, tab_id, _) = store_with_ref();
        let blocker = store
            .block_tab_for_unknown_navigation("sess-a", &target_id, &tab_id, "")
            .expect("known tab");
        let initial = blocker.to_value(Instant::now());
        let blocker_id = initial["blocker_id"].as_str().unwrap().to_owned();
        assert_eq!(initial["kind"], "navigation_outcome_unknown");
        assert_eq!(initial["origin"], Value::Null);

        let reported = store
            .active_tab_navigation_blocker("sess-a", &target_id, &tab_id, "https://reached.example")
            .unwrap()
            .to_value(Instant::now());
        assert_eq!(reported["blocker_id"], blocker_id);
        assert_eq!(reported["origin"], "https://reached.example");
        assert!(matches!(
            store.clear_tab_navigation_blocker_if_matches(
                "sess-a",
                &target_id,
                &tab_id,
                "https://reached.example",
                "blocker-stale",
            ),
            Err(ClearOriginBlockerFailure::Mismatch(_))
        ));
        assert!(store
            .clear_tab_navigation_blocker_if_matches(
                "sess-a",
                &target_id,
                &tab_id,
                "https://reached.example",
                &blocker_id,
            )
            .is_ok());
        assert!(
            store
                .active_tab_navigation_blocker(
                    "sess-a",
                    &target_id,
                    &tab_id,
                    "https://reached.example",
                )
                .is_none()
        );
    }

    #[test]
    fn rate_limit_blocker_reports_server_retry_window() {
        let store = BrowserStore::new();
        let blocker = store.block_origin_for_rate_limit(
            "session-a",
            "https://blocked.example",
            Some(Duration::from_secs(30)),
        );
        let value = blocker.to_value(Instant::now());

        assert_eq!(value["kind"], "rate_limited");
        assert_eq!(value["origin"], "https://blocked.example");
        assert_eq!(value["detection_source"], "http_status");
        assert_eq!(value["server_directed_retry"], true);
        assert_eq!(value["server_retry_after_capped"], false);
        assert!(value["retry_after_ms"]
            .as_u64()
            .is_some_and(|ms| ms <= 30_000));
    }

    #[test]
    fn elapsed_rate_limit_window_allows_the_origin_without_resume() {
        let store = BrowserStore::new();
        store.block_origin_for_rate_limit(
            "session-a",
            "https://blocked.example",
            Some(Duration::ZERO),
        );

        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_none());
    }

    #[test]
    fn fallback_rate_limit_backoff_is_bounded_and_increases() {
        let store = BrowserStore::new();
        let first = store.block_origin_for_rate_limit("session-a", "https://blocked.example", None);
        let second =
            store.block_origin_for_rate_limit("session-a", "https://blocked.example", None);
        let now = Instant::now();
        let first_ms = first.to_value(now)["retry_after_ms"].as_u64().unwrap();
        let second_value = second.to_value(now);
        let second_ms = second_value["retry_after_ms"].as_u64().unwrap();
        let first_id = first.to_value(now)["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second_id = second_value["blocker_id"].as_str().unwrap().to_owned();

        assert!((1_900..=2_500).contains(&first_ms), "{first_ms}");
        assert!((3_900..=5_000).contains(&second_ms), "{second_ms}");
        assert!(second_ms > first_ms);
        assert_eq!(second_value["attempt"], 2);
        assert_eq!(second_value["server_directed_retry"], false);
        assert_ne!(first_id, second_id);
        let failure = store
            .clear_origin_blocker_if_matches(
                "session-a",
                "https://blocked.example",
                &first_id,
                None,
            )
            .expect_err("an earlier 429 capability must not clear a later retry window");
        let ClearOriginBlockerFailure::Mismatch(current) = failure else {
            panic!("expected blocker-id mismatch");
        };
        assert_eq!(current.to_value(now)["blocker_id"], second_id);
    }

    #[test]
    fn stale_blocker_id_refuses_without_clearing_the_current_incident() {
        let store = BrowserStore::new();
        let blocker = store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Semantic,
            Some(&document_identity("loader-1")),
        );

        let failure = store
            .clear_origin_blocker_if_matches(
                "session-a",
                "https://blocked.example",
                "blocker-stale",
                Some(&document_identity("loader-1")),
            )
            .unwrap_err();
        let ClearOriginBlockerFailure::Mismatch(current) = failure else {
            panic!("expected blocker-id mismatch");
        };
        assert_eq!(
            current.to_value(Instant::now())["blocker_id"],
            blocker.to_value(Instant::now())["blocker_id"]
        );
        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_some());
    }

    #[test]
    fn enormous_server_retry_window_is_finite_and_observable() {
        let store = BrowserStore::new();
        let blocker = store.block_origin_for_rate_limit(
            "session-a",
            "https://blocked.example",
            Some(Duration::from_secs(u64::MAX)),
        );
        let value = blocker.to_value(Instant::now());

        assert_eq!(value["server_retry_after_capped"], true);
        assert!(value["retry_after_ms"]
            .as_u64()
            .is_some_and(|ms| ms <= 24 * 60 * 60 * 1_000));
        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_some());
    }

    #[test]
    fn successful_response_clears_only_an_elapsed_rate_limit() {
        let store = BrowserStore::new();
        store.block_origin_for_rate_limit(
            "session-a",
            "https://recovered.example",
            Some(Duration::ZERO),
        );
        assert!(
            store.clear_elapsed_rate_limit_after_success("session-a", "https://recovered.example")
        );
        let next =
            store.block_origin_for_rate_limit("session-a", "https://recovered.example", None);
        assert_eq!(next.to_value(Instant::now())["attempt"], 1);

        store.block_origin_for_challenge(
            "session-a",
            "https://challenge.example",
            BrowserChallengeSource::Semantic,
            Some(&document_identity("loader-1")),
        );
        assert!(
            !store.clear_elapsed_rate_limit_after_success("session-a", "https://challenge.example")
        );
        assert_eq!(
            store
                .active_origin_blocker("session-a", "https://challenge.example")
                .unwrap()
                .to_value(Instant::now())["kind"],
            "anti_bot_challenge"
        );
    }

    #[test]
    fn rate_limit_cannot_replace_an_active_challenge() {
        let store = BrowserStore::new();
        store.block_origin_for_challenge(
            "session-a",
            "https://blocked.example",
            BrowserChallengeSource::Semantic,
            Some(&document_identity("loader-1")),
        );
        let blocker = store.block_origin_for_rate_limit(
            "session-a",
            "https://blocked.example",
            Some(Duration::from_secs(30)),
        );

        assert_eq!(
            blocker.to_value(Instant::now())["kind"],
            "anti_bot_challenge"
        );
        assert_eq!(
            store
                .active_origin_blocker("session-a", "https://blocked.example")
                .unwrap()
                .to_value(Instant::now())["kind"],
            "anti_bot_challenge"
        );
    }

    #[test]
    fn capacity_prunes_elapsed_rate_limits_before_active_blockers() {
        let store = BrowserStore::new();
        for index in 0..(MAX_BLOCKED_ORIGINS_PER_SESSION - 1) {
            store.block_origin_for_challenge(
                "session-a",
                &format!("https://challenge-{index}.example"),
                BrowserChallengeSource::Semantic,
                None,
            );
        }
        store.block_origin_for_rate_limit(
            "session-a",
            "https://elapsed.example",
            Some(Duration::ZERO),
        );

        let admitted = store.block_origin_for_challenge(
            "session-a",
            "https://new.example",
            BrowserChallengeSource::Semantic,
            None,
        );
        assert_eq!(
            admitted.to_value(Instant::now())["kind"],
            "anti_bot_challenge"
        );

        let inner = store.inner.lock().unwrap();
        let blockers = &inner["session-a"].origin_blockers;
        assert_eq!(blockers.len(), MAX_BLOCKED_ORIGINS_PER_SESSION);
        assert!(!blockers.contains_key("https://elapsed.example"));
        assert!(blockers.contains_key("https://challenge-0.example"));
        assert!(blockers.contains_key("https://new.example"));
    }

    #[test]
    fn all_active_capacity_fails_the_session_closed_without_eviction() {
        let store = BrowserStore::new();
        for index in 0..MAX_BLOCKED_ORIGINS_PER_SESSION {
            store.block_origin_for_challenge(
                "session-a",
                &format!("https://challenge-{index}.example"),
                BrowserChallengeSource::Semantic,
                None,
            );
        }

        store.block_origin_for_challenge(
            "session-a",
            "https://new.example",
            BrowserChallengeSource::Semantic,
            None,
        );

        {
            let inner = store.inner.lock().unwrap();
            let session = &inner["session-a"];
            assert_eq!(
                session.origin_blockers.len(),
                MAX_BLOCKED_ORIGINS_PER_SESSION
            );
            assert!(session
                .origin_blockers
                .contains_key("https://challenge-0.example"));
            assert!(!session.origin_blockers.contains_key("https://new.example"));
            assert!(session.blocker_capacity_id.is_some());
        }

        for origin in [
            "https://challenge-0.example",
            "https://new.example",
            "https://otherwise-unblocked.example",
        ] {
            let blocker = store
                .active_origin_blocker("session-a", origin)
                .expect("capacity latch must block every origin");
            let value = blocker.to_value(Instant::now());
            assert_eq!(value["kind"], "safety_capacity", "{origin}: {value}");
            assert_eq!(
                value["handling"], "end_session_before_more_browser_actions",
                "{origin}: {value}"
            );
            assert!(blocker.requires_session_end());
        }
        let first_id = store
            .active_origin_blocker("session-a", "https://challenge-0.example")
            .unwrap()
            .to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other_id = store
            .active_origin_blocker("session-a", "https://otherwise-unblocked.example")
            .unwrap()
            .to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            first_id, other_id,
            "the session capacity incident has one id"
        );
        let capacity_id = store
            .active_origin_blocker("session-a", "https://challenge-0.example")
            .unwrap()
            .to_value(Instant::now())["blocker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(matches!(
            store.clear_origin_blocker_if_matches(
                "session-a",
                "https://challenge-0.example",
                &capacity_id,
                None,
            ),
            Err(ClearOriginBlockerFailure::SessionEndRequired(_))
        ));

        store.remove_session("session-a");
        assert!(store
            .active_origin_blocker("session-a", "https://new.example")
            .is_none());
    }
}
