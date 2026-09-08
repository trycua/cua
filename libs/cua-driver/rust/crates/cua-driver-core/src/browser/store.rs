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
#[derive(Debug, Clone)]
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
pub struct SemanticContinuation {
    pub offset: usize,
    pub query: Option<String>,
    pub scope_backend_node_id: Option<i64>,
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
}

const MAX_BLOCKED_ORIGINS_PER_SESSION: usize = 64;
const RATE_LIMIT_BACKOFF_BASE: Duration = Duration::from_secs(2);
const RATE_LIMIT_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const RATE_LIMIT_SERVER_RETRY_MAX: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginBlockerKind {
    AntiBotChallenge,
    RateLimited,
}

#[derive(Debug, Clone)]
pub(crate) struct OriginBlocker {
    origin: String,
    kind: OriginBlockerKind,
    detection_source: &'static str,
    detected_at: Instant,
    retry_at: Option<Instant>,
    attempt: u32,
    server_directed: bool,
    server_retry_after_capped: bool,
}

impl OriginBlocker {
    fn active(&self, now: Instant) -> bool {
        match self.kind {
            OriginBlockerKind::AntiBotChallenge => true,
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
            OriginBlockerKind::AntiBotChallenge => (
                "anti_bot_challenge",
                true,
                "explicit_resume_or_user_handoff",
            ),
            OriginBlockerKind::RateLimited => {
                ("rate_limited", false, "wait_until_retry_or_explicit_resume")
            }
        };
        json!({
            "required": true,
            "kind": kind,
            "origin": self.origin,
            "detection_source": self.detection_source,
            "retry_after_ms": retry_after_ms,
            "requires_user": requires_user,
            "handling": handling,
            "attempt": self.attempt,
            "server_directed_retry": self.server_directed,
            "server_retry_after_capped": self.server_retry_after_capped,
        })
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

    /// Drop every snapshot of one tab (navigation invalidates refs).
    pub fn invalidate_tab_snapshots(&self, session: &str, target_id: &str, tab_id: &str) {
        self.update_target(session, target_id, |rec| {
            if let Some(tab) = rec.tabs.get_mut(tab_id) {
                tab.snapshots.clear();
            }
        });
    }

    pub(crate) fn block_origin_for_challenge(&self, session: &str, origin: &str) -> OriginBlocker {
        let now = Instant::now();
        let blocker = OriginBlocker {
            origin: origin.to_owned(),
            kind: OriginBlockerKind::AntiBotChallenge,
            detection_source: "page_state",
            detected_at: now,
            retry_at: None,
            attempt: 1,
            server_directed: false,
            server_retry_after_capped: false,
        };
        self.insert_origin_blocker(session, origin, blocker.clone());
        blocker
    }

    pub(crate) fn block_origin_for_rate_limit(
        &self,
        session: &str,
        origin: &str,
        server_retry_after: Option<Duration>,
    ) -> OriginBlocker {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.entry(session.to_owned()).or_default();
        if let Some(challenge) = session_targets
            .origin_blockers
            .get(origin)
            .filter(|blocker| blocker.kind == OriginBlockerKind::AntiBotChallenge)
        {
            return challenge.clone();
        }
        let attempt = session_targets
            .origin_blockers
            .get(origin)
            .filter(|blocker| blocker.kind == OriginBlockerKind::RateLimited)
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
            origin: origin.to_owned(),
            kind: OriginBlockerKind::RateLimited,
            detection_source: "http_status",
            detected_at: now,
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
        insert_bounded_origin_blocker(session_targets, origin, blocker.clone());
        blocker
    }

    pub(crate) fn active_origin_blocker(
        &self,
        session: &str,
        origin: &str,
    ) -> Option<OriginBlocker> {
        let now = Instant::now();
        self.inner
            .lock()
            .unwrap()
            .get(session)
            .and_then(|targets| targets.origin_blockers.get(origin))
            .filter(|blocker| blocker.active(now))
            .cloned()
    }

    pub(crate) fn clear_origin_blocker(&self, session: &str, origin: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get_mut(session)
            .and_then(|targets| targets.origin_blockers.remove(origin))
            .is_some()
    }

    /// Forget an elapsed rate-limit incident after a successful document
    /// response. Active pauses and challenge blockers are never cleared here.
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

    fn insert_origin_blocker(&self, session: &str, origin: &str, blocker: OriginBlocker) {
        let mut inner = self.inner.lock().unwrap();
        let session_targets = inner.entry(session.to_owned()).or_default();
        insert_bounded_origin_blocker(session_targets, origin, blocker);
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
) {
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
        if let Some(oldest) = session
            .origin_blockers
            .iter()
            .min_by_key(|(_, blocker)| blocker.detected_at)
            .map(|(origin, _)| origin.clone())
        {
            session.origin_blockers.remove(&oldest);
        }
    }
    session.origin_blockers.insert(origin.to_owned(), blocker);
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
        store.block_origin_for_challenge("session-a", "https://blocked.example");

        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_some());
        assert!(store
            .active_origin_blocker("session-a", "https://other.example")
            .is_none());
        assert!(store
            .active_origin_blocker("session-b", "https://blocked.example")
            .is_none());
        assert!(store.clear_origin_blocker("session-a", "https://blocked.example"));
        assert!(store
            .active_origin_blocker("session-a", "https://blocked.example")
            .is_none());
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

        assert!((1_900..=2_500).contains(&first_ms), "{first_ms}");
        assert!((3_900..=5_000).contains(&second_ms), "{second_ms}");
        assert!(second_ms > first_ms);
        assert_eq!(second_value["attempt"], 2);
        assert_eq!(second_value["server_directed_retry"], false);
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

        store.block_origin_for_challenge("session-a", "https://challenge.example");
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
        store.block_origin_for_challenge("session-a", "https://blocked.example");
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
            );
        }
        store.block_origin_for_rate_limit(
            "session-a",
            "https://elapsed.example",
            Some(Duration::ZERO),
        );

        store.block_origin_for_challenge("session-a", "https://new.example");

        let inner = store.inner.lock().unwrap();
        let blockers = &inner["session-a"].origin_blockers;
        assert_eq!(blockers.len(), MAX_BLOCKED_ORIGINS_PER_SESSION);
        assert!(!blockers.contains_key("https://elapsed.example"));
        assert!(blockers.contains_key("https://challenge-0.example"));
        assert!(blockers.contains_key("https://new.example"));
    }
}
