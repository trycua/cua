//! Session-scoped target / tab / page-ref capability store.
//!
//! Browser target ids (`bt-<uuid>`), tab ids (`tab-<uuid>`) and page refs
//! (`p<snapshot>:<index>`) are opaque, session-scoped capabilities:
//! they only resolve in the session that minted them, and the whole
//! namespace is dropped when that session ends. Ids are minted from a
//! process-global counter so they never collide across sessions — a
//! capability leaked into another session simply fails to resolve.
//!
//! Refs map internally to CDP `backendNodeId`s. Navigation invalidates
//! every snapshot of the navigated tab; stale refs refuse with
//! `browser_ref_stale`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use uuid::Uuid;

use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::{BindingQuality, ProcessFingerprint, Rect};

/// One interactive element captured in a page snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct RefEntry {
    /// CDP backendNodeId — internal only, never exposed to callers.
    #[serde(skip_serializing)]
    pub backend_node_id: i64,
    pub node_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub id: u64,
    pub url: String,
    /// index → entry; the external ref is `p<id>:<index>`.
    pub refs: HashMap<u32, RefEntry>,
}

#[derive(Debug, Clone)]
pub struct TabRecord {
    pub tab_id: String,
    pub cdp_target_id: String,
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
    pub fingerprint: ProcessFingerprint,
    pub native_title: String,
    pub native_bounds: Rect,
    pub cdp_target_id: String,
    pub cdp_window_id: i64,
    pub quality: BindingQuality,
    pub tabs: HashMap<String, TabRecord>,
}

#[derive(Default)]
struct SessionTargets {
    targets: HashMap<String, TargetRecord>,
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
    next_id: AtomicU64,
}

impl BrowserStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
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
            .and_then(|s| s.refs.get(&idx))
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

    /// Drop every snapshot of one tab (navigation invalidates refs).
    pub fn invalidate_tab_snapshots(&self, session: &str, target_id: &str, tab_id: &str) {
        self.update_target(session, target_id, |rec| {
            if let Some(tab) = rec.tabs.get_mut(tab_id) {
                tab.snapshots.clear();
            }
        });
    }

    /// Drop the whole namespace for an ended session. Wired to
    /// `session::register_session_end_hook` by the engine.
    pub fn remove_session(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
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
            fingerprint: ProcessFingerprint {
                pid: 42,
                start_time: Some(1),
                executable: None,
            },
            native_title: "Docs - Chrome".into(),
            native_bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
            cdp_target_id: "CDP1".into(),
            cdp_window_id: 11,
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
                },
            );
            rec.tabs.insert(
                tab_id.clone(),
                TabRecord {
                    tab_id: tab_id.clone(),
                    cdp_target_id: "CDP1".into(),
                    snapshots: HashMap::from([(
                        snap_id,
                        SnapshotRecord {
                            id: snap_id,
                            url: "https://example.test".into(),
                            refs,
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
    fn minted_ids_are_unique_across_sessions() {
        let store = BrowserStore::new();
        let a = store.mint_target("s1", record());
        let b = store.mint_target("s2", record());
        assert_ne!(a, b, "capability ids must never collide across sessions");
    }
}
