//! BrowserEngine — the semantic core behind the five browser tools.
//!
//! Owns the target/ref store, the CDP connection pool, and every
//! exact-or-refused decision. The platform adapter is consulted for OS
//! identity only; nothing here trusts a cached fact across a mutation
//! boundary — [`BrowserEngine::revalidate_for_mutation`] re-proves the
//! full chain (process fingerprint → native ownership/bounds → endpoint
//! ownership → CDP target type/window) before any input or navigation,
//! and [`BrowserEngine::frame_session_for_mutation`] additionally
//! re-proves the ref's frame/document identity (frame id + loader id,
//! and for OOPIFs the child target attached beneath the proven tab)
//! before the ref is touched.
//!
//! Snapshot composition (v2 DOM-ref slice):
//! - Shadow DOM is composed into the main-frame walk (`pierce: true`),
//!   skipping user-agent shadow roots.
//! - Same-process iframes are walked via `contentDocument` and their
//!   refs carry the child frame's identity from `Page.getFrameTree`.
//! - OOPIFs are reached only when `Target.setAutoAttach` capability-
//!   tests successfully on the tab's own session; child sessions are
//!   accepted solely from `Target.attachedToTarget` events scoped to
//!   that session (containment beneath the proven tab target).
//! - Whenever a frame's identity cannot be proven, its content is
//!   omitted from the snapshot — never guessed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::session::register_session_end_hook;

use super::binding::{
    cardinality_exact_candidate, correlate, selected_tab_target_id, BindingOutcome,
    CdpWindowCandidate,
};
use super::cdp_ws::{CdpConnection, CdpPool};
use super::grant::{ExistingProfileGrant, ExistingProfileGrants, GrantLookup};
use super::mutation::{MutationGates, MutationKey};
use super::platform::{BrowserConsentOutcome, BrowserConsentRequest, BrowserPlatform};
use super::prepare::ManagedBrowsers;
use super::reconnect::ReconnectGates;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::semantic::{
    build_dom_index, build_layout_index, compose_accessibility_tree, parse_viewport,
    OmissionCounts, SemanticDocument, SemanticNode, DEFAULT_SEMANTIC_NODE_BUDGET,
    SEMANTIC_COMPUTED_STYLES,
};
use super::store::{
    format_ref, BrowserStore, FrameIdentity, FrameKind, FrameRef, RefEntry, SemanticContinuation,
    SnapshotRecord, TabRecord, TargetRecord,
};
use super::types::{
    BindingQuality, BrowserClassification, BrowserEngineFamily, NativeWindowInfo, OwnedEndpoint,
    Rect,
};

/// Bounds tolerance (device pixels) for native ↔ CDP window correlation.
/// Absorbs window-shadow and DIP-rounding differences.
pub const BOUNDS_TOLERANCE_PX: f64 = 8.0;

/// Cap on refs minted per snapshot — keeps snapshots bounded on
/// pathological pages. The truncation is reported in the tool output.
pub const MAX_REFS_PER_SNAPSHOT: usize = 300;

pub struct BrowserEngine {
    pub(crate) platform: Arc<dyn BrowserPlatform>,
    pub(crate) store: BrowserStore,
    pub(crate) pool: CdpPool,
    pub(crate) managed_browsers: ManagedBrowsers,
    pub(crate) existing_profile_grants: ExistingProfileGrants,
    pub(crate) approval_broker: crate::consent::ApprovalBroker,
    mutation_gates: MutationGates,
    reconnect_gates: ReconnectGates,
}

fn refuse(code: BrowserRefusalCode, msg: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, msg)
}

fn authorize_live_browser_origin(
    manifest: Option<&crate::session_manifest::SessionManifest>,
    live_url: &str,
) -> Result<(), BrowserRefusal> {
    let manifest = manifest.ok_or_else(|| {
        refuse(
            BrowserRefusalCode::BrowserOriginOutsideScope,
            "the autonomous session policy is unavailable",
        )
    })?;
    manifest
        .authorize_browser_url(live_url)
        .map_err(|error| refuse(BrowserRefusalCode::BrowserOriginOutsideScope, error))
}

fn route_err(context: &str, err: impl std::fmt::Display) -> BrowserRefusal {
    refuse(
        BrowserRefusalCode::BrowserRouteUnavailable,
        format!("{context}: {err}"),
    )
}

pub(super) fn unsupported_engine_refusal(
    classification: &BrowserClassification,
    operation: &'static str,
) -> BrowserRefusal {
    let (message, protocol, limitation) = match classification.engine {
        BrowserEngineFamily::Gecko => (
            "Firefox browser tools require a WebDriver BiDi or Remote Agent route; attaching to an ordinary running profile is not supported",
            "webdriver_bidi",
            "remote_agent_requires_launch_time_enablement",
        ),
        BrowserEngineFamily::Webkit => (
            "Safari browser tools require a WebKit-native automation route; Safari does not expose an attachable CDP endpoint for an ordinary running profile",
            "webkit_automation",
            "no_attachable_runtime_endpoint",
        ),
        BrowserEngineFamily::Chromium => (
            "this Chromium-family browser does not expose a supported CDP route",
            "cdp",
            "cdp_unavailable",
        ),
        BrowserEngineFamily::Unknown => (
            "this browser engine has no supported typed browser route",
            "unknown",
            "engine_route_unavailable",
        ),
    };
    BrowserRefusal::new(BrowserRefusalCode::BrowserRouteUnavailable, message).with_detail(json!({
        "operation": operation,
        "engine_family": classification.engine,
        "product": classification.product_kind,
        "required_protocol": protocol,
        "limitation": limitation,
    }))
}

/// Everything revalidation proves before a mutation proceeds. The
/// `record`/`tab` evidence rides along for future callers even though
/// the v1 tools only need the attached connection.
#[allow(dead_code)]
pub(crate) struct ValidatedTab {
    pub conn: Arc<CdpConnection>,
    pub record: TargetRecord,
    pub tab: TabRecord,
    /// Flattened CDP session id attached to the tab's target.
    pub cdp_session: String,
}

/// Whether a CDP error is Chromium's "method not implemented" shape.
/// Everything else stays a hard failure — a transient error must never
/// be misread as a capability gap.
fn is_method_unsupported(error: &anyhow::Error) -> bool {
    error.to_string().contains("(-32601)")
}

fn is_semantic_document_size_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    [
        "maximum depth",
        "object reference chain is too long",
        "message is too large",
        "serialization",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn is_semantic_document_fallback_error(error: &anyhow::Error) -> bool {
    if is_semantic_document_size_error(error) {
        return true;
    }
    is_semantic_document_timeout_error(error)
}

fn is_semantic_document_timeout_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("cdp dom.getdocument timed out after")
}

const SEMANTIC_DOM_FALLBACK_DEPTHS: &[i64] = &[256, 128, 64, 32, 16, 8, 4, 2, 1];
const SEMANTIC_DOM_TIMEOUT_FALLBACK_DEPTHS: &[i64] = &[8, 4, 2, 1];
const SEMANTIC_DOM_HYDRATION_DEPTH: i64 = 8;
const MAX_SEMANTIC_DOM_HYDRATION_CALLS: usize = 64;
const MAX_SEMANTIC_DOM_SCAN_NODES: usize = 50_000;

#[derive(Default)]
struct DomCoverageScan {
    truncated: Vec<i64>,
    visited_nodes: usize,
    budget_exhausted: bool,
}

fn scan_dom_coverage(value: &Value, scan: &mut DomCoverageScan) {
    if scan.budget_exhausted {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                scan_dom_coverage(value, scan);
            }
        }
        Value::Object(object) => {
            if object.contains_key("nodeType") {
                scan.visited_nodes += 1;
                if scan.visited_nodes > MAX_SEMANTIC_DOM_SCAN_NODES {
                    scan.budget_exhausted = true;
                    return;
                }
                let expected = object
                    .get("childNodeCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                let present = object
                    .get("children")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if expected > present {
                    if let Some(backend_node_id) =
                        object.get("backendNodeId").and_then(Value::as_i64)
                    {
                        scan.truncated.push(backend_node_id);
                    }
                }
            }
            for value in object.values() {
                scan_dom_coverage(value, scan);
            }
        }
        _ => {}
    }
}

fn replace_dom_node(value: &mut Value, backend_node_id: i64, replacement: &Value) -> bool {
    match value {
        Value::Array(values) => values
            .iter_mut()
            .any(|value| replace_dom_node(value, backend_node_id, replacement)),
        Value::Object(object) => {
            if object.get("backendNodeId").and_then(Value::as_i64) == Some(backend_node_id) {
                *value = replacement.clone();
                return true;
            }
            object
                .values_mut()
                .any(|value| replace_dom_node(value, backend_node_id, replacement))
        }
        _ => false,
    }
}

/// Result of one tab snapshot: minted refs plus what was (and was not)
/// composable.
pub(crate) struct SnapshotOutcome {
    pub snapshot_id: u64,
    pub url: String,
    pub refs: Vec<(String, RefEntry)>,
    pub truncated: bool,
    pub oopif: OopifStatus,
}

pub(crate) struct SemanticListedRef {
    pub external: String,
    pub node: SemanticNode,
}

pub(crate) struct SemanticSnapshotOutcome {
    pub snapshot_id: u64,
    pub url: String,
    pub title: String,
    pub outline: String,
    pub refs: Vec<SemanticListedRef>,
    pub content_refs: Vec<SemanticListedRef>,
    pub complete: bool,
    pub scope: &'static str,
    pub selected_nodes: usize,
    pub total_nodes: usize,
    pub omissions: OmissionCounts,
    pub continuation: Option<String>,
    pub oopif: OopifStatus,
}

/// Whether OOPIF content could be composed into the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OopifStatus {
    /// Capability proven; `n` child frames were snapshotted beneath the
    /// tab's own session.
    Attached(usize),
    /// The capability (auto-attach and/or frame-tree identity) could
    /// not be proven — OOPIF content was omitted, not guessed.
    Unsupported,
}

impl OopifStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn frames(&self) -> usize {
        match self {
            Self::Attached(n) => *n,
            Self::Unsupported => 0,
        }
    }
}

/// One CDP session's local frame tree: frame id → loader id. OOPIF
/// children live in their own sessions and do NOT appear here — each
/// session proves exactly its own frames.
pub(crate) struct LocalFrameTree {
    main_frame_id: String,
    frames: HashMap<String, String>,
}

impl LocalFrameTree {
    fn main_identity(&self) -> FrameIdentity {
        FrameIdentity {
            frame_id: self.main_frame_id.clone(),
            loader_id: self.frames[&self.main_frame_id].clone(),
        }
    }

    fn identity_of(&self, frame_id: &str) -> Option<FrameIdentity> {
        self.frames.get(frame_id).map(|loader_id| FrameIdentity {
            frame_id: frame_id.to_owned(),
            loader_id: loader_id.clone(),
        })
    }

    /// Whether a snapshot-time identity still names a live document.
    fn proves(&self, identity: &FrameIdentity) -> bool {
        self.frames.get(&identity.frame_id) == Some(&identity.loader_id)
    }

    fn identities(&self) -> Vec<FrameIdentity> {
        let mut identities = self
            .frames
            .iter()
            .map(|(frame_id, loader_id)| FrameIdentity {
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
            })
            .collect::<Vec<_>>();
        identities.sort_by_key(|identity| identity.frame_id != self.main_frame_id);
        identities
    }
}

/// Parse a `Page.getFrameTree` result. Frames missing an id or loader
/// id are omitted (their refs will be omitted / refused, never
/// guessed); a malformed root fails the whole parse.
fn parse_frame_tree(v: &Value) -> Option<LocalFrameTree> {
    fn walk(node: &Value, frames: &mut HashMap<String, String>) -> Option<String> {
        let frame = node.get("frame")?;
        let id = frame.get("id").and_then(Value::as_str)?.to_owned();
        let loader = frame.get("loaderId").and_then(Value::as_str)?.to_owned();
        frames.insert(id.clone(), loader);
        if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
            for child in children {
                let _ = walk(child, frames);
            }
        }
        Some(id)
    }
    let mut frames = HashMap::new();
    let main_frame_id = walk(v.get("frameTree")?, &mut frames)?;
    Some(LocalFrameTree {
        main_frame_id,
        frames,
    })
}

pub(crate) enum FrameTreeError {
    /// The endpoint does not implement `Page.getFrameTree` (embedded
    /// engines). Frame identity is unprovable; composition degrades to
    /// the v1 main-frame-only behavior.
    Unsupported,
    /// A real failure (transport, malformed reply).
    Failed(anyhow::Error),
}

/// One OOPIF child target attached (flattened) beneath a tab session.
pub(crate) struct AttachedChildFrame {
    pub session_id: String,
    pub target_id: String,
    #[allow(dead_code)]
    pub url: String,
}

pub(crate) enum AttachError {
    /// `Target.setAutoAttach` is not implemented on this session.
    Unsupported,
    Failed(anyhow::Error),
}

impl BrowserEngine {
    /// Create the engine and wire session-end cleanup for the
    /// capability store. Platform crates call this once and register
    /// the five tools via `register_browser_tools`.
    pub fn new(platform: Arc<dyn BrowserPlatform>) -> Arc<Self> {
        Self::new_with_protected_consent_provider(platform, None)
    }

    /// Create an engine with a provider installed by a trusted embedding host
    /// or platform adapter. Ordinary MCP/CLI callers cannot register or replace
    /// this provider after daemon startup.
    pub fn new_with_protected_consent_provider(
        platform: Arc<dyn BrowserPlatform>,
        provider: Option<Arc<dyn crate::consent::ProtectedConsentProvider>>,
    ) -> Arc<Self> {
        let engine = Arc::new(Self {
            platform,
            store: BrowserStore::new(),
            pool: CdpPool::new(),
            managed_browsers: Default::default(),
            existing_profile_grants: ExistingProfileGrants::new(),
            approval_broker: crate::consent::ApprovalBroker::new(provider),
            mutation_gates: MutationGates::new(),
            reconnect_gates: ReconnectGates::new(),
        });
        let weak: Weak<Self> = Arc::downgrade(&engine);
        register_session_end_hook(move |session_id| {
            if let Some(engine) = weak.upgrade() {
                engine.store.remove_session(session_id);
                engine.cleanup_prepared_session(session_id);
                for grant in engine.existing_profile_grants.remove_session(session_id) {
                    engine.pool.release_claim_marker(&grant.endpoint_ws_url);
                    if let Some(protected) = grant.protected_consent.as_ref() {
                        protected.indicator.revoke();
                    }
                    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                        let engine = engine.clone();
                        runtime.spawn(async move {
                            engine
                                .pool
                                .release_existing(&grant.endpoint_ws_url, grant.generation)
                                .await;
                            if let Some(protected) = grant.protected_consent.as_ref() {
                                engine.approval_broker.revoke(protected).await;
                            }
                        });
                    }
                }
            }
        });
        engine
    }

    // ── Endpoint / CDP plumbing ─────────────────────────────────────────

    pub(crate) async fn existing_profile_grant(
        &self,
        session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) -> Result<Option<ExistingProfileGrant>, BrowserRefusal> {
        match self
            .existing_profile_grants
            .lookup(session, transport_session, pid)
        {
            GrantLookup::Missing => Ok(None),
            GrantLookup::Live(grant) => Ok(Some(grant)),
            GrantLookup::Expired(grant) => {
                self.pool.release_claim_marker(&grant.endpoint_ws_url);
                self.pool
                    .release_existing(&grant.endpoint_ws_url, grant.generation)
                    .await;
                if let Some(protected) = grant.protected_consent.as_ref() {
                    self.approval_broker.revoke(protected).await;
                }
                Err(refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "the existing-profile grant expired; approve this attachment again",
                ))
            }
        }
    }

    pub(crate) async fn revoke_existing_profile_grant(
        &self,
        session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) {
        if let Some(grant) = self
            .existing_profile_grants
            .revoke(session, transport_session, pid)
        {
            self.pool.release_claim_marker(&grant.endpoint_ws_url);
            self.pool
                .release_existing(&grant.endpoint_ws_url, grant.generation)
                .await;
            if let Some(protected) = grant.protected_consent.as_ref() {
                self.approval_broker.revoke(protected).await;
            }
        }
    }

    pub(crate) async fn connect(&self, ws_url: &str) -> Result<Arc<CdpConnection>, BrowserRefusal> {
        match self.pool.get(ws_url).await {
            Ok(conn) => Ok(conn),
            Err(first_err) => {
                // One redial after eviction covers a browser restart on
                // the same port; a second failure is a real refusal.
                self.pool.evict(ws_url).await;
                self.pool
                    .get(ws_url)
                    .await
                    .map_err(|_| route_err("cannot connect to owned DevTools endpoint", first_err))
            }
        }
    }

    async fn connect_existing_profile(
        &self,
        session: &str,
        transport_session: Option<&str>,
        pid: i64,
    ) -> Result<(Arc<CdpConnection>, ExistingProfileGrant), BrowserRefusal> {
        let grant = self
            .existing_profile_grant(session, transport_session, pid)
            .await?
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "no live existing-profile grant remains for this browser session",
                )
            })?;
        if let Ok(conn) = self
            .pool
            .get_existing(&grant.endpoint_ws_url, grant.generation)
            .await
        {
            return Ok((conn, grant));
        }

        // One leader owns endpoint reproof and bounded redial. Followers
        // re-check the generation after acquiring this gate and reuse its
        // socket rather than opening another browser-level connection.
        let _leader = self
            .reconnect_gates
            .lock(&grant.fingerprint, &grant.endpoint_ws_url)
            .await;
        let mut grant = self
            .existing_profile_grant(session, transport_session, pid)
            .await?
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "the existing-profile grant ended while reconnecting",
                )
            })?;
        if let Ok(conn) = self
            .pool
            .get_existing(&grant.endpoint_ws_url, grant.generation)
            .await
        {
            return Ok((conn, grant));
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(32);
        let mut last_error = None;
        while tokio::time::Instant::now() < deadline && grant.reconnect_attempts_remaining > 0 {
            if grant.pid != pid || grant.browser != "chromium" {
                self.revoke_existing_profile_grant(session, transport_session, pid)
                    .await;
                return Err(refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "the reconnect request no longer matches the approved browser identity",
                ));
            }
            let classification = self.platform.classify_browser(pid).await?;
            if !classification.supports_cdp
                || classification.engine != super::types::BrowserEngineFamily::Chromium
            {
                self.revoke_existing_profile_grant(session, transport_session, pid)
                    .await;
                return Err(refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "the approved process is no longer a supported Chromium browser",
                ));
            }
            let fingerprint = self.platform.process_fingerprint(pid).await?;
            if !grant.fingerprint.matches(&fingerprint) {
                self.revoke_existing_profile_grant(session, transport_session, pid)
                    .await;
                return Err(refuse(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "the browser process changed; existing-profile attachment needs fresh approval",
                ));
            }
            let endpoint = self
                .platform
                .reprove_existing_profile_endpoint(pid, &grant.endpoint_ws_url)
                .await?
                .ok_or_else(|| {
                    refuse(
                        BrowserRefusalCode::BrowserRequiresSetup,
                        "the approved browser endpoint disappeared during reconnect",
                    )
                })?;
            if endpoint.ownership.owner_pid != pid {
                return Err(refuse(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the reconnect endpoint is not owned by the approved browser process",
                ));
            }
            if endpoint.ws_url != grant.endpoint_ws_url {
                self.revoke_existing_profile_grant(session, transport_session, pid)
                    .await;
                return Err(refuse(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the browser DevTools endpoint changed during reconnect",
                ));
            }

            let old_generation = grant.generation;
            let new_generation =
                self.existing_profile_grants
                    .bump_generation(session, transport_session, pid)?;
            self.store
                .invalidate_endpoint_generation(pid, old_generation);
            let attempt = super::grant::MAX_RECONNECT_ATTEMPTS
                .saturating_sub(grant.reconnect_attempts_remaining)
                .saturating_add(1);
            let mut reconnect = Box::pin(self.pool.reconnect_existing(
                &endpoint.ws_url,
                old_generation,
                new_generation,
            ));
            let reconnected = tokio::select! {
                result = &mut reconnect => result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    match self.platform.handle_existing_profile_consent(BrowserConsentRequest {
                        pid,
                        window_id: grant.window_id,
                        attempt,
                    }).await {
                        Ok(BrowserConsentOutcome::Accepted | BrowserConsentOutcome::NotPresent) => {
                            reconnect.await
                        }
                        Err(error) => {
                            // The reconnect future may be waiting on browser
                            // consent. Cancel it before grant revocation so no
                            // socket-pool resource can outlive this refusal.
                            drop(reconnect);
                            if error.code == BrowserRefusalCode::BrowserConsentRevoked {
                                self.revoke_existing_profile_grant(session, transport_session, pid)
                                    .await;
                            }
                            return Err(error);
                        }
                    }
                }
            };
            match reconnected {
                Ok(conn) => {
                    grant = self
                        .existing_profile_grant(session, transport_session, pid)
                        .await?
                        .expect("grant exists after successful generation bump");
                    return Ok((conn, grant));
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    grant = self
                        .existing_profile_grant(session, transport_session, pid)
                        .await?
                        .expect("grant exists while reconnect budget remains");
                }
            }
        }
        self.revoke_existing_profile_grant(session, transport_session, pid)
            .await;
        Err(refuse(
            BrowserRefusalCode::BrowserReconnectExhausted,
            "the bounded existing-profile reconnect attempts did not establish a proven browser socket",
        )
        .with_detail(json!({
            "attempt_limit": super::grant::MAX_RECONNECT_ATTEMPTS,
            "last_error": last_error.map(|_| "connection_failed"),
            "retryable": false,
        })))
    }

    async fn connection_for_record(
        &self,
        session: &str,
        record: &TargetRecord,
    ) -> Result<Arc<CdpConnection>, BrowserRefusal> {
        if record.generation == 0 {
            return self.connect(&record.ws_url).await;
        }
        let (conn, grant) = self
            .connect_existing_profile(
                session,
                record.grant_transport_session.as_deref(),
                record.pid,
            )
            .await?;
        if grant.generation != record.generation {
            return Err(refuse(
                BrowserRefusalCode::BrowserBindingStale,
                "the browser reconnected and invalidated this target; re-run get_browser_state",
            ));
        }
        Ok(conn)
    }

    /// Discover + ownership-check the endpoint for `pid`.
    pub(crate) async fn owned_endpoint(&self, pid: i64) -> Result<OwnedEndpoint, BrowserRefusal> {
        let endpoint = self
            .platform
            .discover_owned_endpoint(pid)
            .await?
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserRequiresSetup,
                    format!(
                        "no owned DevTools endpoint for pid {pid} — run browser_prepare \
                     explicitly to set one up"
                    ),
                )
            })?;
        if endpoint.ownership.owner_pid != pid {
            return Err(refuse(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                format!(
                    "endpoint ownership proof attributes the endpoint to pid {} but the \
                     target is pid {pid}",
                    endpoint.ownership.owner_pid
                ),
            ));
        }
        Ok(endpoint)
    }

    /// Re-prove the endpoint exposed by an explicitly approved existing
    /// profile. This route is intentionally separate from driver-managed
    /// endpoint discovery: Chrome's per-instance remote-debugging toggle can
    /// expose a PID-owned listener whose exact WebSocket path is available
    /// only through the browser's default-profile DevToolsActivePort file.
    async fn existing_profile_endpoint(
        &self,
        pid: i64,
        expected_ws_url: &str,
    ) -> Result<OwnedEndpoint, BrowserRefusal> {
        let endpoint = self
            .platform
            .reprove_existing_profile_endpoint(pid, expected_ws_url)
            .await?
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserRequiresSetup,
                    "the approved existing-profile DevTools endpoint is no longer available",
                )
            })?;
        if endpoint.ownership.owner_pid != pid {
            return Err(refuse(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the existing-profile endpoint is not owned by the approved browser process",
            ));
        }
        Ok(endpoint)
    }

    /// List page-type CDP targets with their window geometry.
    async fn window_candidates(
        &self,
        conn: &CdpConnection,
    ) -> Result<Vec<CdpWindowCandidate>, BrowserRefusal> {
        let targets = conn
            .call(None, "Target.getTargets", json!({}))
            .await
            .map_err(|e| route_err("Target.getTargets failed", e))?;
        let infos = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "Target.getTargets returned no targetInfos array",
                )
            })?;

        let mut out = Vec::new();
        for info in infos {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let url = info
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if url.starts_with("devtools://") {
                continue;
            }
            let target_id = info
                .get("targetId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    refuse(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        "Target.getTargets returned a page without targetId",
                    )
                })?;
            let title = info
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();

            let window_geometry = match conn
                .call(
                    None,
                    "Browser.getWindowForTarget",
                    json!({ "targetId": target_id }),
                )
                .await
            {
                Ok(win) => {
                    let window_id =
                        win.get("windowId").and_then(Value::as_i64).ok_or_else(|| {
                            refuse(
                                BrowserRefusalCode::BrowserRouteUnavailable,
                                "Browser.getWindowForTarget returned no windowId",
                            )
                        })?;
                    let bounds_v = conn
                        .call(
                            None,
                            "Browser.getWindowBounds",
                            json!({ "windowId": window_id }),
                        )
                        .await
                        .map_err(|error| {
                            route_err(
                                "Browser.getWindowBounds failed while proving the native window",
                                error,
                            )
                        })?;
                    let b = bounds_v.get("bounds").ok_or_else(|| {
                        refuse(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            "Browser.getWindowBounds returned no bounds object",
                        )
                    })?;
                    let number = |field: &str| {
                        b.get(field).and_then(Value::as_f64).ok_or_else(|| {
                            refuse(
                                BrowserRefusalCode::BrowserRouteUnavailable,
                                format!("Browser.getWindowBounds returned no numeric {field}"),
                            )
                        })
                    };
                    Some((
                        window_id,
                        Rect::new(
                            number("left")?,
                            number("top")?,
                            number("width")?,
                            number("height")?,
                        ),
                    ))
                }
                // Electron's browser endpoint can omit this Browser-domain
                // method entirely. Retain only that explicit unsupported
                // shape; every transient/vanished-target error fails the
                // whole proof rather than shrinking it to a false unique set.
                Err(error) if is_method_unsupported(&error) => None,
                Err(error) => {
                    return Err(route_err(
                        "Browser.getWindowForTarget failed while proving the native window",
                        error,
                    ));
                }
            };
            out.push(CdpWindowCandidate {
                cdp_target_id: target_id,
                cdp_window_id: window_geometry.map(|(window_id, _)| window_id),
                title,
                url,
                bounds: window_geometry.map(|(_, bounds)| bounds),
            });
        }
        Ok(out)
    }

    /// Attach (flattened) to a tab's target and return the CDP session id.
    async fn attach(
        &self,
        conn: &CdpConnection,
        cdp_target_id: &str,
    ) -> Result<String, BrowserRefusal> {
        let attached = conn
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": cdp_target_id, "flatten": true }),
            )
            .await
            .map_err(|e| {
                refuse(
                    BrowserRefusalCode::BrowserTabNotFound,
                    format!("cannot attach to tab target {cdp_target_id}: {e}"),
                )
            })?;
        attached
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserTabNotFound,
                    format!("attach to {cdp_target_id} returned no sessionId"),
                )
            })
    }

    // ── Binding (get_browser_state, pid + window_id mode) ──────────────

    /// Classify, inspect, discover, correlate — and mint a target
    /// capability on success. `session` must already be explicit.
    pub(crate) async fn bind_native(
        &self,
        session: &str,
        transport_session: Option<&str>,
        pid: i64,
        window_id: u64,
    ) -> Result<(String, TargetRecord), BrowserRefusal> {
        let class = self.platform.classify_browser(pid).await?;
        if !class.is_browser {
            return Err(refuse(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("pid {pid} is not a recognized browser process"),
            ));
        }
        if !class.supports_cdp {
            return Err(unsupported_engine_refusal(&class, "bind_native_window"));
        }

        let native = self.native_window_checked(pid, window_id).await?;
        let fingerprint = self.platform.process_fingerprint(pid).await?;
        let mut grant = self
            .existing_profile_grant(session, transport_session, pid)
            .await?;
        let endpoint = if let Some(live_grant) = &grant {
            self.existing_profile_endpoint(pid, &live_grant.endpoint_ws_url)
                .await?
        } else {
            self.owned_endpoint(pid).await?
        };
        if let Some(grant) = &grant {
            if !grant.fingerprint.matches(&fingerprint)
                || grant.endpoint_ws_url != endpoint.ws_url
                || grant.window_id != window_id
            {
                return Err(refuse(
                    BrowserRefusalCode::BrowserBindingStale,
                    "the approved browser process, endpoint, or native window changed; approve the existing profile again",
                ));
            }
        }
        let conn = if grant.is_some() {
            let (conn, live_grant) = self
                .connect_existing_profile(session, transport_session, pid)
                .await?;
            grant = Some(live_grant);
            conn
        } else {
            self.connect(&endpoint.ws_url).await?
        };
        let candidates = self.window_candidates(&conn).await?;

        let correlation = correlate(&native, &candidates, BOUNDS_TOLERANCE_PX);
        let (candidate, quality) = match correlation {
            BindingOutcome::Bound {
                candidate,
                quality: BindingQuality::Exact,
            } => (candidate, BindingQuality::Exact),
            BindingOutcome::Bound {
                candidate,
                quality: BindingQuality::Heuristic,
            } => {
                let only_native_window = self
                    .platform
                    .is_only_exact_native_window(pid, window_id)
                    .await?;
                match cardinality_exact_candidate(&native.title, &candidates, only_native_window) {
                    Some(exact) => (exact, BindingQuality::Exact),
                    None => (candidate, BindingQuality::Heuristic),
                }
            }
            BindingOutcome::Ambiguous(candidate_count) => {
                return Err(refuse(
                    BrowserRefusalCode::BrowserBindingAmbiguous,
                    "multiple CDP targets match the native window and the title \
                     tie-break cannot pick a unique one",
                )
                .with_detail(json!({ "candidate_count": candidate_count })));
            }
            BindingOutcome::None => {
                let only_native_window = self
                    .platform
                    .is_only_exact_native_window(pid, window_id)
                    .await?;
                match cardinality_exact_candidate(&native.title, &candidates, only_native_window) {
                    Some(exact) => (exact, BindingQuality::Exact),
                    None => {
                        return Err(refuse(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!(
                                "no CDP target correlates with native window {window_id} of \
                                 pid {pid} — refusing rather than guessing"
                            ),
                        ));
                    }
                }
            }
        };

        // Tabs = page targets living in the bound CDP window. Selection is a
        // separate proof from native-window correlation: a representative CDP
        // target is only a window handle and must never be reported as active.
        let selected_cdp_target_id =
            selected_tab_target_id(&native.title, &candidates, candidate.cdp_window_id);
        let mut tabs = HashMap::new();
        for c in candidates.iter().filter(|c| match candidate.cdp_window_id {
            Some(window_id) => c.cdp_window_id == Some(window_id),
            None => c.cdp_target_id == candidate.cdp_target_id,
        }) {
            let tab_id = self.store.mint_tab_id();
            tabs.insert(
                tab_id.clone(),
                TabRecord {
                    tab_id,
                    cdp_target_id: c.cdp_target_id.clone(),
                    title: c.title.clone(),
                    url: c.url.clone(),
                    active: selected_cdp_target_id.map(|selected| selected == c.cdp_target_id),
                    generation: grant.as_ref().map_or(0, |grant| grant.generation),
                    snapshots: HashMap::new(),
                },
            );
        }

        let record = TargetRecord {
            target_id: String::new(),
            pid,
            window_id,
            ws_url: endpoint.ws_url.clone(),
            endpoint_owner_pid: endpoint.ownership.owner_pid,
            generation: grant.as_ref().map_or(0, |grant| grant.generation),
            grant_transport_session: grant.as_ref().map(|grant| grant.transport_session.clone()),
            fingerprint,
            native_title: native.title.clone(),
            native_bounds: native.bounds,
            cdp_target_id: candidate.cdp_target_id.clone(),
            cdp_window_id: candidate.cdp_window_id,
            quality,
            tabs,
        };
        let target_id = self.store.mint_target(session, record.clone());
        let record = self.store.get_target(session, &target_id)?;
        Ok((target_id, record))
    }

    pub(crate) async fn native_window_checked(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal> {
        let native = self.platform.native_window(pid, window_id).await?;
        if native.ownership.owner_pid != pid {
            return Err(refuse(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "native ownership proof attributes window {window_id} to pid {} — \
                     not the requested pid {pid}",
                    native.ownership.owner_pid
                ),
            ));
        }
        Ok(native)
    }

    // ── Revalidation (before every mutation) ────────────────────────────

    /// Re-prove the entire binding chain for a mutation on one tab.
    /// Exact-or-refused: heuristic bindings never reach the mutation
    /// path.
    pub(crate) async fn revalidate_for_mutation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: Option<&str>,
    ) -> Result<ValidatedTab, BrowserRefusal> {
        let record = self.store.get_target(session, target_id)?;

        if record.quality != BindingQuality::Exact {
            return Err(refuse(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "this binding is heuristic (title-only) — mutations require an exact \
                 bounds- or cardinality-correlated binding",
            ));
        }

        let tab_id = tab_id.ok_or_else(|| {
            refuse(
                BrowserRefusalCode::BrowserTabRequired,
                "this operation requires an explicit tab_id from get_browser_state",
            )
        })?;
        let tab = record.tabs.get(tab_id).cloned().ok_or_else(|| {
            refuse(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;

        if record.generation != tab.generation {
            return Err(refuse(
                BrowserRefusalCode::BrowserBindingStale,
                "the tab capability belongs to an older browser connection generation",
            ));
        }
        if record.generation > 0 {
            let grant = self
                .existing_profile_grant(
                    session,
                    record.grant_transport_session.as_deref(),
                    record.pid,
                )
                .await?
                .ok_or_else(|| {
                    refuse(
                        BrowserRefusalCode::BrowserConsentRequired,
                        "the existing-profile grant ended; approve and bind the browser again",
                    )
                })?;
            if grant.generation != record.generation {
                return Err(refuse(
                    BrowserRefusalCode::BrowserBindingStale,
                    "the browser connection generation changed; re-run get_browser_state",
                ));
            }
        }

        // 1. Process fingerprint — pid reuse / restart detection.
        let fp_now = self.platform.process_fingerprint(record.pid).await?;
        if !record.fingerprint.matches(&fp_now) {
            return Err(refuse(
                BrowserRefusalCode::BrowserBindingStale,
                format!(
                    "process fingerprint for pid {} changed since binding — the browser \
                     restarted or the pid was reused",
                    record.pid
                ),
            ));
        }

        // 2. Native window still exists and is still owned by the pid.
        let native = self
            .native_window_checked(record.pid, record.window_id)
            .await?;

        // 3. Endpoint still owned and unchanged.
        let endpoint = if record.generation > 0 {
            self.existing_profile_endpoint(record.pid, &record.ws_url)
                .await?
        } else {
            self.owned_endpoint(record.pid).await?
        };
        if endpoint.ws_url != record.ws_url {
            return Err(refuse(
                BrowserRefusalCode::BrowserBindingStale,
                "the owned DevTools endpoint changed since binding — re-run \
                 get_browser_state",
            ));
        }

        // 4. CDP target still a page in the bound CDP window, with either
        //    matching geometry or the same singleton cardinality proof.
        let conn = self.connection_for_record(session, &record).await?;
        let candidates = self.window_candidates(&conn).await?;
        let live = candidates
            .iter()
            .find(|c| c.cdp_target_id == tab.cdp_target_id)
            .ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserTabNotFound,
                    format!("tab {tab_id} no longer has a live CDP page target"),
                )
            })?;
        if let Some(bound_window_id) = record.cdp_window_id {
            if live.cdp_window_id != Some(bound_window_id) {
                return Err(refuse(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "the tab moved to a different browser window since binding",
                ));
            }
            let geometry_matches = live
                .bounds
                .is_some_and(|bounds| bounds.approx_eq(&native.bounds, BOUNDS_TOLERANCE_PX));
            let correlation_still_exact = if geometry_matches {
                true
            } else {
                let only_native_window = self
                    .platform
                    .is_only_exact_native_window(record.pid, record.window_id)
                    .await?;
                cardinality_exact_candidate(&native.title, &candidates, only_native_window)
                    .is_some_and(|candidate| candidate.cdp_window_id == Some(bound_window_id))
            };
            if !correlation_still_exact {
                return Err(refuse(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "CDP window no longer has an exact geometry or singleton-cardinality \
                     correlation with the native window — refusing to mutate it",
                ));
            }
        } else if candidates.len() != 1
            || live.cdp_window_id.is_some()
            || self
                .platform
                .is_only_exact_native_window(record.pid, record.window_id)
                .await?
                != Some(true)
        {
            return Err(refuse(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the embedded browser is no longer provably single-page and single-window",
            ));
        }

        let cdp_session = self.attach(&conn, &tab.cdp_target_id).await?;
        if crate::authorization::configured_permission_mode()
            .is_ok_and(|mode| mode == crate::authorization::PermissionMode::Autonomous)
        {
            let frame_tree = conn
                .call(Some(&cdp_session), "Page.getFrameTree", json!({}))
                .await
                .map_err(|error| {
                    route_err(
                        "could not prove the live browser origin before autonomous input",
                        error,
                    )
                })?;
            let live_url = frame_tree
                .pointer("/frameTree/frame/url")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    refuse(
                        BrowserRefusalCode::BrowserOriginOutsideScope,
                        "the live top-level browser origin could not be proven",
                    )
                })?;
            let manifest =
                crate::session_manifest::configured_session_manifest().map_err(|error| {
                    refuse(
                        BrowserRefusalCode::BrowserOriginOutsideScope,
                        format!("the autonomous session policy is unavailable: {error}"),
                    )
                })?;
            authorize_live_browser_origin(manifest, live_url)?;
        }
        Ok(ValidatedTab {
            conn,
            record,
            tab,
            cdp_session,
        })
    }

    /// Serialize the full revalidate-dispatch-verify interval by the real CDP
    /// target rather than by a caller-controlled session id.
    pub(crate) async fn lock_mutation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, BrowserRefusal> {
        let record = self.store.get_target(session, target_id)?;
        let tab = record.tabs.get(tab_id).ok_or_else(|| {
            refuse(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        Ok(self
            .mutation_gates
            .lock(MutationKey::new(&record.fingerprint, &tab.cdp_target_id))
            .await)
    }

    // ── Frame identity / OOPIF plumbing ─────────────────────────────────

    /// Fetch and parse one session's local frame tree.
    async fn local_frame_tree(
        &self,
        conn: &CdpConnection,
        cdp_session: &str,
    ) -> Result<LocalFrameTree, FrameTreeError> {
        match conn
            .call(Some(cdp_session), "Page.getFrameTree", json!({}))
            .await
        {
            Ok(v) => parse_frame_tree(&v).ok_or_else(|| {
                FrameTreeError::Failed(anyhow::anyhow!(
                    "Page.getFrameTree returned a malformed frame tree"
                ))
            }),
            Err(e) if is_method_unsupported(&e) => Err(FrameTreeError::Unsupported),
            Err(e) => Err(FrameTreeError::Failed(e)),
        }
    }

    /// Capability-tested OOPIF discovery: enable flattened auto-attach
    /// on `tab_session` and collect the iframe children Chromium
    /// announces for existing targets *before* the setAutoAttach ack.
    ///
    /// Containment: only `Target.attachedToTarget` events scoped to
    /// this exact tab session are honored, and only `type == "iframe"`
    /// targets — a child announced on any other session (or a popup
    /// page target) is ignored. Each operation attaches its own fresh
    /// tab session, so the parent-session filter is per-operation
    /// unique even on the shared pooled connection.
    async fn attached_iframe_children(
        &self,
        conn: &CdpConnection,
        tab_session: &str,
    ) -> Result<Vec<AttachedChildFrame>, AttachError> {
        // Subscribe BEFORE issuing the command so pre-ack events are
        // guaranteed to be queued when the call returns.
        let mut events = conn.subscribe();
        match conn
            .call(
                Some(tab_session),
                "Target.setAutoAttach",
                json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
            )
            .await
        {
            Ok(_) => {}
            Err(e) if is_method_unsupported(&e) => return Err(AttachError::Unsupported),
            Err(e) => return Err(AttachError::Failed(e)),
        }
        let mut out = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.method != "Target.attachedToTarget" {
                continue;
            }
            if event.session_id.as_deref() != Some(tab_session) {
                continue; // not contained beneath the proven tab session
            }
            let info = &event.params["targetInfo"];
            if info["type"].as_str() != Some("iframe") {
                continue; // OOPIF slice covers iframes only, never popups/workers
            }
            let (Some(session_id), Some(target_id)) = (
                event.params["sessionId"].as_str(),
                info["targetId"].as_str(),
            ) else {
                continue;
            };
            out.push(AttachedChildFrame {
                session_id: session_id.to_owned(),
                target_id: target_id.to_owned(),
                url: info["url"].as_str().unwrap_or("").to_owned(),
            });
        }
        Ok(out)
    }

    /// Re-prove a ref's frame/document identity and return the CDP
    /// session its `backendNodeId` is valid in. Called after
    /// [`Self::revalidate_for_mutation`], before the ref is touched.
    /// Any identity that cannot be re-proven is a refusal, and a stale
    /// document additionally invalidates the tab's snapshots.
    pub(crate) async fn frame_session_for_mutation(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        validated: &ValidatedTab,
        frame: &FrameRef,
    ) -> Result<String, BrowserRefusal> {
        let conn = &validated.conn;
        let stale = |message: &str| {
            self.store
                .invalidate_tab_snapshots(session, target_id, tab_id);
            refuse(BrowserRefusalCode::BrowserRefStale, message)
        };
        let tree_err = |e: FrameTreeError| match e {
            FrameTreeError::Unsupported => refuse(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "the browser no longer reports its frame tree — the ref's frame identity \
                 cannot be re-proven",
            ),
            FrameTreeError::Failed(err) => {
                route_err("Page.getFrameTree failed during frame revalidation", err)
            }
        };

        match &frame.oopif_target_id {
            None => {
                // v1-compat main-frame refs minted without a frame tree
                // carry no identity to re-check; node liveness (box
                // model / focus failures map to stale) is the backstop.
                let Some(identity) = &frame.identity else {
                    return Ok(validated.cdp_session.clone());
                };
                let tree = self
                    .local_frame_tree(conn, &validated.cdp_session)
                    .await
                    .map_err(tree_err)?;
                if tree.proves(identity) {
                    Ok(validated.cdp_session.clone())
                } else {
                    Err(stale(
                        "the ref's frame navigated or was removed since the snapshot — \
                         re-run get_browser_state to re-snapshot",
                    ))
                }
            }
            Some(oopif_target) => {
                let Some(identity) = &frame.identity else {
                    // Unreachable by construction (OOPIF refs are only
                    // minted with identity); refuse defensively.
                    return Err(refuse(
                        BrowserRefusalCode::BrowserRefStale,
                        "the OOPIF ref carries no provable frame identity",
                    ));
                };
                let children = self
                    .attached_iframe_children(conn, &validated.cdp_session)
                    .await
                    .map_err(|e| match e {
                        AttachError::Unsupported => refuse(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            "the browser no longer supports capability-tested OOPIF \
                             attachment — the ref's cross-process frame cannot be re-proven",
                        ),
                        AttachError::Failed(err) => {
                            route_err("Target.setAutoAttach failed during frame revalidation", err)
                        }
                    })?;
                let Some(child) = children.into_iter().find(|c| c.target_id == *oopif_target)
                else {
                    return Err(stale(
                        "the ref's cross-process frame is no longer attached beneath the \
                         bound tab — re-run get_browser_state to re-snapshot",
                    ));
                };
                let tree = self
                    .local_frame_tree(conn, &child.session_id)
                    .await
                    .map_err(tree_err)?;
                if tree.proves(identity) {
                    Ok(child.session_id)
                } else {
                    Err(stale(
                        "the ref's cross-process frame navigated since the snapshot — \
                         re-run get_browser_state to re-snapshot",
                    ))
                }
            }
        }
    }

    // ── Read-side: page snapshot (ref minting) ──────────────────────────

    /// Snapshot one tab's composed DOM (main frame + shadow DOM +
    /// same-process iframes + capability-tested OOPIFs) and mint
    /// `p<snap>:<index>` refs for interactive elements. Read-only
    /// against the page.
    pub(crate) async fn snapshot_tab(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
    ) -> Result<SnapshotOutcome, BrowserRefusal> {
        let record = self.store.get_target(session, target_id)?;
        let tab = record.tabs.get(tab_id).cloned().ok_or_else(|| {
            refuse(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        let conn = self.connection_for_record(session, &record).await?;
        let cdp_session = self.attach(&conn, &tab.cdp_target_id).await?;

        let doc = conn
            .call(
                Some(&cdp_session),
                "DOM.getDocument",
                json!({ "depth": -1, "pierce": true }),
            )
            .await
            .map_err(|e| route_err("DOM.getDocument failed", e))?;
        let root = doc.get("root").cloned().unwrap_or(Value::Null);
        let url = root
            .get("documentURL")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        // Frame/document identity for the tab session's local frames.
        // Unsupported degrades to the v1-compat main-frame-only path.
        let local_tree = match self.local_frame_tree(&conn, &cdp_session).await {
            Ok(tree) => Some(tree),
            Err(FrameTreeError::Unsupported) => None,
            Err(FrameTreeError::Failed(e)) => return Err(route_err("Page.getFrameTree failed", e)),
        };

        let mut collected = Vec::new();
        let root_frame_id = root
            .get("frameId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        collect_interactive(&root, root_frame_id.as_deref(), true, &mut collected);

        let mut entries: Vec<RefEntry> = Vec::new();
        for c in collected {
            let frame = match (c.in_root_frame, &local_tree) {
                (true, Some(tree)) => FrameRef {
                    kind: FrameKind::Main,
                    oopif_target_id: None,
                    identity: Some(tree.main_identity()),
                },
                (true, None) => FrameRef::main_unproven(),
                (false, Some(tree)) => {
                    let Some(identity) = c
                        .frame_id
                        .as_deref()
                        .and_then(|frame_id| tree.identity_of(frame_id))
                    else {
                        continue; // frame identity unprovable → omit
                    };
                    FrameRef {
                        kind: FrameKind::Iframe,
                        oopif_target_id: None,
                        identity: Some(identity),
                    }
                }
                // No frame tree → iframe identity unprovable → omit.
                (false, None) => continue,
            };
            entries.push(RefEntry {
                backend_node_id: c.backend_node_id,
                node_name: c.node_name,
                label: c.label,
                actions: Vec::new(),
                visibility: None,
                semantic: false,
                frame,
            });
        }

        // OOPIF children (C2): only with a provable frame tree AND a
        // successful capability test, contained beneath this tab's own
        // session. Anything unprovable is omitted, never guessed.
        let oopif = if local_tree.is_some() {
            match self.attached_iframe_children(&conn, &cdp_session).await {
                Ok(children) => {
                    let mut attached = 0usize;
                    for child in &children {
                        let Ok(child_tree) = self.local_frame_tree(&conn, &child.session_id).await
                        else {
                            continue; // identity unprovable → omit this frame
                        };
                        let Ok(child_doc) = conn
                            .call(
                                Some(&child.session_id),
                                "DOM.getDocument",
                                json!({ "depth": -1, "pierce": true }),
                            )
                            .await
                        else {
                            continue;
                        };
                        let child_root = child_doc.get("root").cloned().unwrap_or(Value::Null);
                        let child_root_frame = child_root
                            .get("frameId")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        let mut child_collected = Vec::new();
                        collect_interactive(
                            &child_root,
                            child_root_frame.as_deref(),
                            true,
                            &mut child_collected,
                        );
                        for c in child_collected {
                            let identity = if c.in_root_frame {
                                Some(child_tree.main_identity())
                            } else {
                                c.frame_id
                                    .as_deref()
                                    .and_then(|frame_id| child_tree.identity_of(frame_id))
                            };
                            let Some(identity) = identity else { continue };
                            entries.push(RefEntry {
                                backend_node_id: c.backend_node_id,
                                node_name: c.node_name,
                                label: c.label,
                                actions: Vec::new(),
                                visibility: None,
                                semantic: false,
                                frame: FrameRef {
                                    kind: FrameKind::Oopif,
                                    oopif_target_id: Some(child.target_id.clone()),
                                    identity: Some(identity),
                                },
                            });
                        }
                        attached += 1;
                    }
                    // Contain the child sessions this read minted:
                    // stop auto-attaching (best effort).
                    let _ = conn
                        .call(
                            Some(&cdp_session),
                            "Target.setAutoAttach",
                            json!({
                                "autoAttach": false,
                                "waitForDebuggerOnStart": false,
                                "flatten": true
                            }),
                        )
                        .await;
                    for child in &children {
                        let _ = conn
                            .call(
                                Some(&cdp_session),
                                "Target.detachFromTarget",
                                json!({ "sessionId": child.session_id }),
                            )
                            .await;
                    }
                    OopifStatus::Attached(attached)
                }
                Err(AttachError::Unsupported) => OopifStatus::Unsupported,
                Err(AttachError::Failed(e)) => {
                    return Err(route_err("Target.setAutoAttach failed", e))
                }
            }
        } else {
            OopifStatus::Unsupported
        };

        let truncated = entries.len() > MAX_REFS_PER_SNAPSHOT;
        entries.truncate(MAX_REFS_PER_SNAPSHOT);
        if truncated {
            tracing::warn!(
                "browser snapshot truncated to {MAX_REFS_PER_SNAPSHOT} refs for tab {tab_id}"
            );
        }

        let snapshot_id = self.store.mint_snapshot_id();
        let mut refs = HashMap::new();
        let mut listed = Vec::new();
        for (i, entry) in entries.into_iter().enumerate() {
            let idx = i as u32;
            listed.push((format_ref(snapshot_id, idx), entry.clone()));
            refs.insert(idx, entry);
        }
        // A new snapshot supersedes prior ones for the tab: only the
        // latest namespace stays resolvable, so refs can never mix
        // across snapshots of a mutating page.
        self.store.update_target(session, target_id, |rec| {
            if let Some(tab) = rec.tabs.get_mut(tab_id) {
                tab.snapshots.clear();
                tab.snapshots.insert(
                    snapshot_id,
                    SnapshotRecord {
                        id: snapshot_id,
                        generation: record.generation,
                        url: url.clone(),
                        refs,
                        semantic: None,
                        semantic_root_identity: None,
                        continuations: HashMap::new(),
                    },
                );
            }
        });
        Ok(SnapshotOutcome {
            snapshot_id,
            url,
            refs: listed,
            truncated,
            oopif,
        })
    }

    async fn collect_semantic_session(
        &self,
        conn: &Arc<CdpConnection>,
        cdp_session: &str,
        document: &Value,
        tree: Option<&LocalFrameTree>,
        oopif_target_id: Option<&str>,
    ) -> Result<SemanticDocument, BrowserRefusal> {
        let root = document.get("root").cloned().unwrap_or(Value::Null);
        let styles = SEMANTIC_COMPUTED_STYLES
            .iter()
            .map(|value| Value::String((*value).to_owned()))
            .collect::<Vec<_>>();
        let (layout, metrics) = tokio::try_join!(
            conn.call(
                Some(cdp_session),
                "DOMSnapshot.captureSnapshot",
                json!({
                    "computedStyles": styles,
                    "includePaintOrder": true,
                    "includeDOMRects": true
                }),
            ),
            conn.call(Some(cdp_session), "Page.getLayoutMetrics", json!({})),
        )
        .map_err(|error| route_err("semantic layout collection failed", error))?;
        let dom = build_dom_index(&root);
        let layout = build_layout_index(&layout);
        let viewport = parse_viewport(&metrics);

        let identities = tree.map(LocalFrameTree::identities).unwrap_or_default();
        let mut result = SemanticDocument::default();
        if identities.is_empty() {
            let ax = conn
                .call(Some(cdp_session), "Accessibility.getFullAXTree", json!({}))
                .await
                .map_err(|error| route_err("Accessibility.getFullAXTree failed", error))?;
            result = compose_accessibility_tree(
                &ax,
                &dom,
                &layout,
                &viewport,
                FrameRef::main_unproven(),
            );
        } else {
            for (index, identity) in identities.into_iter().enumerate() {
                let ax = conn
                    .call(
                        Some(cdp_session),
                        "Accessibility.getFullAXTree",
                        json!({ "frameId": identity.frame_id }),
                    )
                    .await
                    .map_err(|error| route_err("Accessibility.getFullAXTree failed", error))?;
                let kind = if oopif_target_id.is_some() {
                    FrameKind::Oopif
                } else if index == 0 {
                    FrameKind::Main
                } else {
                    FrameKind::Iframe
                };
                let mut frame_document = compose_accessibility_tree(
                    &ax,
                    &dom,
                    &layout,
                    &viewport,
                    FrameRef {
                        kind,
                        oopif_target_id: oopif_target_id.map(str::to_owned),
                        identity: Some(identity),
                    },
                );
                if index > 0 {
                    frame_document.css_hidden_dom_count = 0;
                }
                result.extend(frame_document);
            }
        }
        Ok(result)
    }

    async fn semantic_document(
        &self,
        conn: &Arc<CdpConnection>,
        cdp_session: &str,
    ) -> Result<(Value, bool), BrowserRefusal> {
        match conn
            .call(
                Some(cdp_session),
                "DOM.getDocument",
                json!({ "depth": -1, "pierce": true }),
            )
            .await
        {
            Ok(document) => Ok((document, true)),
            Err(error) if is_semantic_document_fallback_error(&error) => {
                let mut last_size_error = error.to_string();
                let fallback_depths = if is_semantic_document_timeout_error(&error) {
                    SEMANTIC_DOM_TIMEOUT_FALLBACK_DEPTHS
                } else {
                    SEMANTIC_DOM_FALLBACK_DEPTHS
                };
                for depth in fallback_depths {
                    match conn
                        .call(
                            Some(cdp_session),
                            "DOM.getDocument",
                            json!({ "depth": depth, "pierce": true }),
                        )
                        .await
                    {
                        Ok(document) => {
                            let document = self
                                .hydrate_semantic_document(conn, cdp_session, document)
                                .await?;
                            return Ok((document, false));
                        }
                        Err(error) if is_semantic_document_fallback_error(&error) => {
                            last_size_error = error.to_string();
                        }
                        Err(error) => {
                            return Err(route_err("bounded DOM.getDocument fallback failed", error))
                        }
                    }
                }
                Err(route_err(
                    "bounded DOM.getDocument fallback exhausted every accepted depth",
                    last_size_error,
                ))
            }
            Err(error) => Err(route_err("DOM.getDocument failed", error)),
        }
    }

    async fn hydrate_semantic_document(
        &self,
        conn: &Arc<CdpConnection>,
        cdp_session: &str,
        mut document: Value,
    ) -> Result<Value, BrowserRefusal> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut attempted = HashSet::new();
        for _ in 0..MAX_SEMANTIC_DOM_HYDRATION_CALLS {
            let mut scan = DomCoverageScan::default();
            scan_dom_coverage(&document, &mut scan);
            if scan.budget_exhausted {
                break;
            }
            let Some(backend_node_id) = scan
                .truncated
                .into_iter()
                .find(|backend_node_id| attempted.insert(*backend_node_id))
            else {
                break;
            };
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let response = match tokio::time::timeout(
                deadline - now,
                conn.call(
                    Some(cdp_session),
                    "DOM.describeNode",
                    json!({
                        "backendNodeId": backend_node_id,
                        "depth": SEMANTIC_DOM_HYDRATION_DEPTH,
                        "pierce": true
                    }),
                ),
            )
            .await
            {
                Err(_) => break,
                Ok(Ok(response)) => response,
                Ok(Err(error)) if is_semantic_document_size_error(&error) => continue,
                Ok(Err(error)) => {
                    return Err(route_err("DOM.describeNode hydration failed", error))
                }
            };
            let Some(node) = response.get("node") else {
                continue;
            };
            let _ = replace_dom_node(&mut document, backend_node_id, node);
        }
        Ok(document)
    }

    fn semantic_outcome(
        &self,
        snapshot_id: u64,
        url: String,
        title: String,
        page: super::semantic::SemanticPage,
        document_complete: bool,
        scope: &'static str,
        oopif: OopifStatus,
        start_index: u32,
    ) -> (SemanticSnapshotOutcome, HashMap<u32, RefEntry>) {
        let mut stored_refs = HashMap::new();
        let mut refs = Vec::new();
        let mut content_refs = Vec::new();
        for (position, node) in page.selected.iter().enumerate() {
            let Some(entry) = node.to_ref_entry() else {
                continue;
            };
            let index = start_index.saturating_add(position as u32);
            let external = format_ref(snapshot_id, index);
            stored_refs.insert(index, entry);
            let listed = SemanticListedRef {
                external,
                node: node.clone(),
            };
            if node.actions.is_empty() {
                content_refs.push(listed);
            } else {
                refs.push(listed);
            }
        }
        let complete = document_complete && page.next_offset.is_none();
        (
            SemanticSnapshotOutcome {
                snapshot_id,
                url,
                title,
                outline: page.outline,
                refs,
                content_refs,
                complete,
                scope,
                selected_nodes: page.selected_nodes,
                total_nodes: page.total_nodes,
                omissions: page.omissions,
                continuation: page.next_offset.map(|_| format!("bc-{}", Uuid::new_v4())),
                oopif,
            },
            stored_refs,
        )
    }

    pub(crate) async fn snapshot_tab_semantic(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        scope_ref: Option<&str>,
        query: Option<&str>,
        continuation: Option<&str>,
    ) -> Result<SemanticSnapshotOutcome, BrowserRefusal> {
        if let Some(token) = continuation {
            if scope_ref.is_some() || query.is_some() {
                return Err(refuse(
                    BrowserRefusalCode::BrowserRefStale,
                    "continuation cannot be combined with a new scope_ref or query",
                ));
            }
            let (snapshot, continuation) = self
                .store
                .resolve_semantic_continuation(session, target_id, tab_id, token)?;
            let record = self.store.get_target(session, target_id)?;
            let tab = record.tabs.get(tab_id).cloned().ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserTabNotFound,
                    format!("tab {tab_id} is not known for target {target_id}"),
                )
            })?;
            if let Some(identity) = &snapshot.semantic_root_identity {
                let conn = self.connection_for_record(session, &record).await?;
                let cdp_session = self.attach(&conn, &tab.cdp_target_id).await?;
                let tree = self.local_frame_tree(&conn, &cdp_session).await.map_err(|error| {
                    match error {
                        FrameTreeError::Unsupported => refuse(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            "the browser no longer reports its frame tree, so the semantic \n+                             continuation's document identity cannot be re-proven",
                        ),
                        FrameTreeError::Failed(error) => route_err(
                            "Page.getFrameTree failed during semantic continuation revalidation",
                            error,
                        ),
                    }
                })?;
                if !tree.proves(identity) {
                    self.store
                        .invalidate_tab_snapshots(session, target_id, tab_id);
                    return Err(refuse(
                        BrowserRefusalCode::BrowserRefStale,
                        "the page navigated since this semantic continuation was minted; \n+                         re-run get_browser_state to start a fresh snapshot",
                    ));
                }
            }
            let document = snapshot.semantic.clone().ok_or_else(|| {
                refuse(
                    BrowserRefusalCode::BrowserRefStale,
                    "the continuation no longer has semantic snapshot state",
                )
            })?;
            let page = document.page(
                continuation.offset,
                DEFAULT_SEMANTIC_NODE_BUDGET,
                continuation.query.as_deref(),
                continuation.scope_backend_node_id,
            );
            let start_index = snapshot
                .refs
                .keys()
                .max()
                .copied()
                .map_or(0, |value| value.saturating_add(1));
            let oopif = if continuation.oopif_supported {
                OopifStatus::Attached(continuation.oopif_frames)
            } else {
                OopifStatus::Unsupported
            };
            let next_offset = page.next_offset;
            let (outcome, new_refs) = self.semantic_outcome(
                snapshot.id,
                snapshot.url.clone(),
                tab.title,
                page,
                document.complete,
                "continuation",
                oopif,
                start_index,
            );
            let next_token = outcome.continuation.clone();
            self.store.update_target(session, target_id, |record| {
                if let Some(stored) = record
                    .tabs
                    .get_mut(tab_id)
                    .and_then(|tab| tab.snapshots.get_mut(&snapshot.id))
                {
                    stored.continuations.remove(token);
                    stored.refs.extend(new_refs);
                    if let (Some(token), Some(offset)) = (next_token, next_offset) {
                        stored.continuations.insert(
                            token,
                            SemanticContinuation {
                                offset,
                                query: continuation.query.clone(),
                                scope_backend_node_id: continuation.scope_backend_node_id,
                                oopif_supported: continuation.oopif_supported,
                                oopif_frames: continuation.oopif_frames,
                            },
                        );
                    }
                }
            });
            return Ok(outcome);
        }

        let scope_backend_node_id = match scope_ref {
            Some(external) => Some(
                self.store
                    .resolve_ref(session, target_id, tab_id, external)?
                    .backend_node_id,
            ),
            None => None,
        };
        let record = self.store.get_target(session, target_id)?;
        let tab = record.tabs.get(tab_id).cloned().ok_or_else(|| {
            refuse(
                BrowserRefusalCode::BrowserTabNotFound,
                format!("tab {tab_id} is not known for target {target_id}"),
            )
        })?;
        let conn = self.connection_for_record(session, &record).await?;
        let cdp_session = self.attach(&conn, &tab.cdp_target_id).await?;
        let (document, document_complete) = self.semantic_document(&conn, &cdp_session).await?;
        let root = document.get("root").cloned().unwrap_or(Value::Null);
        let url = root
            .get("documentURL")
            .and_then(Value::as_str)
            .unwrap_or(&tab.url)
            .to_owned();
        let local_tree = match self.local_frame_tree(&conn, &cdp_session).await {
            Ok(tree) => Some(tree),
            Err(FrameTreeError::Unsupported) => None,
            Err(FrameTreeError::Failed(error)) => {
                return Err(route_err("Page.getFrameTree failed", error))
            }
        };
        let semantic_root_identity = local_tree.as_ref().map(LocalFrameTree::main_identity);
        let mut semantic = self
            .collect_semantic_session(&conn, &cdp_session, &document, local_tree.as_ref(), None)
            .await?;
        semantic.complete &= document_complete;

        let oopif = if local_tree.is_some() {
            match self.attached_iframe_children(&conn, &cdp_session).await {
                Ok(children) => {
                    let mut attached = 0;
                    for child in &children {
                        let child_tree = match self.local_frame_tree(&conn, &child.session_id).await
                        {
                            Ok(tree) => tree,
                            Err(_) => {
                                semantic.unprovable_frame_count += 1;
                                semantic.complete = false;
                                continue;
                            }
                        };
                        let (child_document, child_complete) =
                            match self.semantic_document(&conn, &child.session_id).await {
                                Ok(document) => document,
                                Err(_) => {
                                    semantic.unprovable_frame_count += 1;
                                    semantic.complete = false;
                                    continue;
                                }
                            };
                        match self
                            .collect_semantic_session(
                                &conn,
                                &child.session_id,
                                &child_document,
                                Some(&child_tree),
                                Some(&child.target_id),
                            )
                            .await
                        {
                            Ok(document) => {
                                let mut document = document;
                                document.complete &= child_complete;
                                semantic.extend(document);
                                attached += 1;
                            }
                            Err(_) => {
                                semantic.unprovable_frame_count += 1;
                                semantic.complete = false;
                            }
                        }
                    }
                    let _ = conn
                        .call(
                            Some(&cdp_session),
                            "Target.setAutoAttach",
                            json!({
                                "autoAttach": false,
                                "waitForDebuggerOnStart": false,
                                "flatten": true
                            }),
                        )
                        .await;
                    for child in &children {
                        let _ = conn
                            .call(
                                Some(&cdp_session),
                                "Target.detachFromTarget",
                                json!({ "sessionId": child.session_id }),
                            )
                            .await;
                    }
                    OopifStatus::Attached(attached)
                }
                Err(AttachError::Unsupported) => OopifStatus::Unsupported,
                Err(AttachError::Failed(error)) => {
                    return Err(route_err("Target.setAutoAttach failed", error))
                }
            }
        } else {
            OopifStatus::Unsupported
        };

        let page = semantic.page(
            0,
            DEFAULT_SEMANTIC_NODE_BUDGET,
            query,
            scope_backend_node_id,
        );
        let next_offset = page.next_offset;
        let snapshot_id = self.store.mint_snapshot_id();
        let scope = if scope_ref.is_some() {
            "subtree"
        } else if query.is_some() {
            "query"
        } else {
            "viewport"
        };
        let (outcome, refs) = self.semantic_outcome(
            snapshot_id,
            url.clone(),
            tab.title.clone(),
            page,
            semantic.complete,
            scope,
            oopif,
            0,
        );
        let continuation_token = outcome.continuation.clone();
        self.store
            .update_target(session, target_id, |stored_target| {
                if let Some(stored_tab) = stored_target.tabs.get_mut(tab_id) {
                    let mut continuations = HashMap::new();
                    if let (Some(token), Some(offset)) = (continuation_token, next_offset) {
                        continuations.insert(
                            token,
                            SemanticContinuation {
                                offset,
                                query: query.map(str::to_owned),
                                scope_backend_node_id,
                                oopif_supported: matches!(oopif, OopifStatus::Attached(_)),
                                oopif_frames: oopif.frames(),
                            },
                        );
                    }
                    stored_tab.snapshots.clear();
                    stored_tab.snapshots.insert(
                        snapshot_id,
                        SnapshotRecord {
                            id: snapshot_id,
                            generation: record.generation,
                            url,
                            refs,
                            semantic: Some(semantic),
                            semantic_root_identity,
                            continuations,
                        },
                    );
                }
            });
        Ok(outcome)
    }
}

/// Attribute names that make an element interactive-enough to ref.
const INTERACTIVE_ATTRS: &[&str] = &["onclick", "role", "contenteditable", "tabindex", "href"];
/// Element names always considered interactive.
const INTERACTIVE_TAGS: &[&str] = &[
    "a", "button", "input", "select", "textarea", "option", "summary", "label",
];
/// Attributes surfaced into the human-readable ref label.
const LABEL_ATTRS: &[&str] = &[
    "aria-label",
    "placeholder",
    "name",
    "id",
    "type",
    "value",
    "href",
    "role",
];

/// One interactive element found by the composed DOM walk, before
/// frame identity has been resolved against the frame tree.
struct CollectedNode {
    backend_node_id: i64,
    node_name: String,
    label: Option<String>,
    /// Frame id of the containing frame when known (from the iframe
    /// element's / document's `frameId`).
    frame_id: Option<String>,
    /// Whether the node lives in the session's root frame (as opposed
    /// to a descended same-process `contentDocument`).
    in_root_frame: bool,
}

/// Walk a pierced `DOM.getDocument` node tree collecting interactive
/// elements in document order. Composition rules:
/// - `shadowRoots` are descended (composed into their host's frame),
///   except user-agent shadow roots (internal control chrome).
/// - Same-process iframes are descended via `contentDocument`, tagged
///   with the child frame's id.
/// - OOPIF placeholders (iframe elements without a `contentDocument`)
///   are NOT descended here — they are reached only through
///   capability-tested child sessions.
fn collect_interactive(
    node: &Value,
    frame_id: Option<&str>,
    in_root_frame: bool,
    out: &mut Vec<CollectedNode>,
) {
    let node_type = node.get("nodeType").and_then(Value::as_i64).unwrap_or(0);
    if node_type == 1 {
        let name = node
            .get("nodeName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let attrs: Vec<String> = node
            .get("attributes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let attr = |key: &str| -> Option<&str> {
            attrs
                .chunks_exact(2)
                .find(|kv| kv[0].eq_ignore_ascii_case(key))
                .map(|kv| kv[1].as_str())
        };
        let interactive = INTERACTIVE_TAGS.contains(&name.as_str())
            || INTERACTIVE_ATTRS.iter().any(|a| attr(a).is_some());
        if interactive {
            if let Some(backend) = node.get("backendNodeId").and_then(Value::as_i64) {
                let label = LABEL_ATTRS
                    .iter()
                    .filter_map(|k| attr(k).map(|v| format!("{k}={v}")))
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push(CollectedNode {
                    backend_node_id: backend,
                    node_name: name,
                    label: (!label.is_empty()).then_some(label),
                    frame_id: frame_id.map(str::to_owned),
                    in_root_frame,
                });
            }
        }
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_interactive(child, frame_id, in_root_frame, out);
        }
    }
    if let Some(shadow_roots) = node.get("shadowRoots").and_then(Value::as_array) {
        for shadow_root in shadow_roots {
            if shadow_root.get("shadowRootType").and_then(Value::as_str) == Some("user-agent") {
                continue;
            }
            collect_interactive(shadow_root, frame_id, in_root_frame, out);
        }
    }
    if let Some(content_document) = node.get("contentDocument") {
        // Same-process iframe. The child frame id lives on the iframe
        // element (and again on the content document); without it the
        // frame's identity is unprovable and its refs get omitted.
        let child_frame_id = node
            .get("frameId")
            .or_else(|| content_document.get("frameId"))
            .and_then(Value::as_str);
        collect_interactive(content_document, child_frame_id, false, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_doc() -> Value {
        json!({
            "nodeType": 9,
            "nodeName": "#document",
            "frameId": "F_MAIN",
            "children": [{
                "nodeType": 1,
                "nodeName": "HTML",
                "backendNodeId": 1,
                "children": [
                    {
                        "nodeType": 1,
                        "nodeName": "BUTTON",
                        "backendNodeId": 10,
                        "attributes": ["aria-label", "Submit"],
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "DIV",
                        "backendNodeId": 11,
                        "shadowRoots": [{
                            "nodeType": 11,
                            "nodeName": "#document-fragment",
                            "shadowRootType": "open",
                            "backendNodeId": 12,
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "BUTTON",
                                "backendNodeId": 20,
                                "attributes": ["aria-label", "Shadow Go"],
                            }]
                        }]
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "INPUT",
                        "backendNodeId": 21,
                        "attributes": ["type", "text"],
                        "shadowRoots": [{
                            "nodeType": 11,
                            "nodeName": "#document-fragment",
                            "shadowRootType": "user-agent",
                            "backendNodeId": 23,
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "DIV",
                                "backendNodeId": 22,
                                "attributes": ["role", "button"],
                            }]
                        }]
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "SPAN",
                        "backendNodeId": 24,
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "IFRAME",
                        "backendNodeId": 13,
                        "frameId": "F_IFRAME",
                        "contentDocument": {
                            "nodeType": 9,
                            "nodeName": "#document",
                            "frameId": "F_IFRAME",
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "BUTTON",
                                "backendNodeId": 30,
                                "attributes": ["id", "inner-btn"],
                            }]
                        }
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "IFRAME",
                        "backendNodeId": 14,
                        "frameId": "F_OOPIF",
                    }
                ]
            }]
        })
    }

    #[test]
    fn collector_composes_shadow_dom_and_same_process_iframes() {
        let doc = fixture_doc();
        let mut out = Vec::new();
        collect_interactive(&doc, Some("F_MAIN"), true, &mut out);
        let backends: Vec<i64> = out.iter().map(|e| e.backend_node_id).collect();
        assert_eq!(
            backends,
            vec![10, 20, 21, 30],
            "main button + open-shadow button + input + same-process iframe button; \
             no span, no user-agent shadow content, no OOPIF placeholder descent"
        );
        assert_eq!(out[0].label.as_deref(), Some("aria-label=Submit"));

        let shadow = out.iter().find(|e| e.backend_node_id == 20).unwrap();
        assert!(
            shadow.in_root_frame,
            "shadow DOM composes into its host frame"
        );
        assert_eq!(shadow.frame_id.as_deref(), Some("F_MAIN"));

        let inner = out.iter().find(|e| e.backend_node_id == 30).unwrap();
        assert!(!inner.in_root_frame);
        assert_eq!(inner.frame_id.as_deref(), Some("F_IFRAME"));
    }

    #[test]
    fn parse_frame_tree_maps_frames_and_omits_malformed_children() {
        let tree = parse_frame_tree(&json!({
            "frameTree": {
                "frame": { "id": "F_MAIN", "loaderId": "L1", "url": "https://a.test/" },
                "childFrames": [
                    { "frame": { "id": "F_CHILD", "loaderId": "L2" } },
                    { "frame": { "id": "F_NO_LOADER" } }
                ]
            }
        }))
        .expect("valid tree");
        assert!(tree.proves(&FrameIdentity {
            frame_id: "F_MAIN".into(),
            loader_id: "L1".into()
        }));
        assert!(tree.proves(&FrameIdentity {
            frame_id: "F_CHILD".into(),
            loader_id: "L2".into()
        }));
        assert!(!tree.proves(&FrameIdentity {
            frame_id: "F_CHILD".into(),
            loader_id: "L9".into()
        }));
        assert!(
            tree.identity_of("F_NO_LOADER").is_none(),
            "a frame without a loader id is unprovable and omitted"
        );
        assert_eq!(tree.main_identity().frame_id, "F_MAIN");

        assert!(
            parse_frame_tree(&json!({ "frameTree": { "frame": { "id": "x" } } })).is_none(),
            "a root without a loader id fails the parse"
        );
        assert!(parse_frame_tree(&json!({})).is_none());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn live_browser_origin_decision_refuses_redirect_outside_manifest_scope() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session-policy.yaml");
        std::fs::write(
            &path,
            r#"
version: 1
mode: autonomous
expires_after: 1h
idle_timeout: 10m
resources:
  browser:
    origins: [https://app.example.com]
allow:
  tools: [browser_navigate, browser_click]
"#,
        )
        .unwrap();
        let manifest = crate::session_manifest::load_manifest(&path).unwrap();

        authorize_live_browser_origin(Some(&manifest), "https://app.example.com/work").unwrap();
        let refusal =
            authorize_live_browser_origin(Some(&manifest), "https://attacker.example/redirected")
                .unwrap_err();
        assert_eq!(refusal.code, BrowserRefusalCode::BrowserOriginOutsideScope);
        assert!(authorize_live_browser_origin(None, "https://app.example.com").is_err());
    }
}
