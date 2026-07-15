//! BrowserEngine — the semantic core behind the five browser tools.
//!
//! Owns the target/ref store, the CDP connection pool, and every
//! exact-or-refused decision. The platform adapter is consulted for OS
//! identity only; nothing here trusts a cached fact across a mutation
//! boundary — [`BrowserEngine::revalidate_for_mutation`] re-proves the
//! full chain (process fingerprint → native ownership/bounds → endpoint
//! ownership → CDP target type/window) before any input or navigation.

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
use super::store::{format_ref, BrowserStore, RefEntry, SnapshotRecord, TabRecord, TargetRecord};
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
                Err(error) if error.to_string().contains("(-32601)") => None,
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

    // ── Read-side: page snapshot (ref minting) ──────────────────────────

    /// Snapshot one tab's main-frame DOM and mint `p<snap>:<index>`
    /// refs for interactive elements. Read-only against the page.
    pub(crate) async fn snapshot_tab(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
    ) -> Result<(u64, String, Vec<(String, RefEntry)>), BrowserRefusal> {
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
                json!({ "depth": -1, "pierce": false }),
            )
            .await
            .map_err(|e| route_err("DOM.getDocument failed", e))?;

        let url = doc
            .get("root")
            .and_then(|r| r.get("documentURL"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        let mut entries: Vec<RefEntry> = Vec::new();
        if let Some(root) = doc.get("root") {
            collect_interactive(root, &mut entries);
        }
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
        Ok((snapshot_id, url, listed))
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

/// Walk a `DOM.getDocument` node tree collecting interactive elements
/// in document order. Main frame only: `contentDocument` children
/// (iframes) are deliberately not descended into for v1.
fn collect_interactive(node: &Value, out: &mut Vec<RefEntry>) {
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
                out.push(RefEntry {
                    backend_node_id: backend,
                    node_name: name,
                    label: (!label.is_empty()).then_some(label),
                });
            }
        }
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_interactive(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_interactive_walks_main_frame_only() {
        let doc = json!({
            "nodeType": 9,
            "nodeName": "#document",
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
                        "attributes": ["role", "checkbox"],
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "SPAN",
                        "backendNodeId": 12,
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "IFRAME",
                        "backendNodeId": 13,
                        "contentDocument": {
                            "nodeType": 9,
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "BUTTON",
                                "backendNodeId": 99
                            }]
                        }
                    }
                ]
            }]
        });
        let mut out = Vec::new();
        collect_interactive(&doc, &mut out);
        let backends: Vec<i64> = out.iter().map(|e| e.backend_node_id).collect();
        assert_eq!(
            backends,
            vec![10, 11],
            "button + role div, no span, no iframe child"
        );
        assert_eq!(out[0].label.as_deref(), Some("aria-label=Submit"));
    }
}
