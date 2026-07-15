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

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use serde_json::{json, Value};

use crate::session::register_session_end_hook;

use super::binding::{
    correlate, embedded_single_page_candidate, BindingOutcome, CdpWindowCandidate,
};
use super::cdp_ws::{CdpConnection, CdpPool};
use super::platform::BrowserPlatform;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::{
    format_ref, BrowserStore, FrameIdentity, FrameKind, FrameRef, RefEntry, SnapshotRecord,
    TabRecord, TargetRecord,
};
use super::types::{BindingQuality, NativeWindowInfo, OwnedEndpoint, Rect};

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
}

fn refuse(code: BrowserRefusalCode, msg: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, msg)
}

fn route_err(context: &str, err: impl std::fmt::Display) -> BrowserRefusal {
    refuse(
        BrowserRefusalCode::BrowserRouteUnavailable,
        format!("{context}: {err}"),
    )
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

/// Result of one tab snapshot: minted refs plus what was (and was not)
/// composable.
pub(crate) struct SnapshotOutcome {
    pub snapshot_id: u64,
    pub url: String,
    pub refs: Vec<(String, RefEntry)>,
    pub truncated: bool,
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
        let engine = Arc::new(Self {
            platform,
            store: BrowserStore::new(),
            pool: CdpPool::new(),
        });
        let weak: Weak<Self> = Arc::downgrade(&engine);
        register_session_end_hook(move |session_id| {
            if let Some(engine) = weak.upgrade() {
                engine.store.remove_session(session_id);
            }
        });
        engine
    }

    // ── Endpoint / CDP plumbing ─────────────────────────────────────────

    async fn connect(&self, ws_url: &str) -> Result<Arc<CdpConnection>, BrowserRefusal> {
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

    /// Discover + ownership-check the endpoint for `pid`.
    async fn owned_endpoint(&self, pid: i64) -> Result<OwnedEndpoint, BrowserRefusal> {
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
            return Err(refuse(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "{} does not expose a CDP route in browser-tool v1",
                    class.product.as_deref().unwrap_or("this browser")
                ),
            ));
        }

        let native = self.native_window_checked(pid, window_id).await?;
        let endpoint = self.owned_endpoint(pid).await?;
        let fingerprint = self.platform.process_fingerprint(pid).await?;
        let conn = self.connect(&endpoint.ws_url).await?;
        let candidates = self.window_candidates(&conn).await?;

        let only_native_window = if candidates.len() == 1
            && candidates[0].cdp_window_id.is_none()
            && candidates[0].bounds.is_none()
        {
            self.platform
                .is_only_exact_native_window(pid, window_id)
                .await?
        } else {
            None
        };
        let embedded = embedded_single_page_candidate(&candidates, only_native_window);
        let (candidate, quality) = if let Some(candidate) = embedded {
            (candidate, BindingQuality::Exact)
        } else {
            match correlate(&native, &candidates, BOUNDS_TOLERANCE_PX) {
                BindingOutcome::Bound { candidate, quality } => (candidate, quality),
                BindingOutcome::Ambiguous(candidate_count) => {
                    return Err(refuse(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        "multiple CDP targets match the native window and the title \
                         tie-break cannot pick a unique one",
                    )
                    .with_detail(json!({ "candidate_count": candidate_count })));
                }
                BindingOutcome::None => {
                    return Err(refuse(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!(
                            "no CDP target correlates with native window {window_id} of \
                             pid {pid} — refusing rather than guessing"
                        ),
                    ));
                }
            }
        };

        // Tabs = page targets living in the bound CDP window.
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

    async fn native_window_checked(
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
                 bounds-correlated binding",
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
        let endpoint = self.owned_endpoint(record.pid).await?;
        if endpoint.ws_url != record.ws_url {
            return Err(refuse(
                BrowserRefusalCode::BrowserBindingStale,
                "the owned DevTools endpoint changed since binding — re-run \
                 get_browser_state",
            ));
        }

        // 4. CDP target still a page in the bound CDP window, with
        //    geometry that still matches the native window.
        let conn = self.connect(&record.ws_url).await?;
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
            if !live
                .bounds
                .is_some_and(|bounds| bounds.approx_eq(&native.bounds, BOUNDS_TOLERANCE_PX))
            {
                return Err(refuse(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "CDP window geometry no longer matches the native window — refusing to \
                     mutate a target that cannot be re-proven",
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
        Ok(ValidatedTab {
            conn,
            record,
            tab,
            cdp_session,
        })
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
        let conn = self.connect(&record.ws_url).await?;
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
                        url: url.clone(),
                        refs,
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
}
