use crate::element_token::{
    format_snapshot_id, parse_token, refusal, ResolvedElement, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
};
use crate::protocol::ToolResult;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub trait SnapshotPayload: Send + Sync + 'static {
    type Element;
    fn len(&self) -> usize;
    fn retain(&self, index: usize) -> Option<Self::Element>;
}

struct Snapshot<S> {
    id: u32,
    window_id: u64,
    screenshot_owner: Option<String>,
    screenshot_scale: Option<f64>,
    zoom: Option<ZoomContext>,
    payload: S,
}

impl<S> Snapshot<S> {
    fn screenshot(&self, session: Option<&str>) -> Option<ScreenshotContext> {
        let scale = self
            .screenshot_scale
            .filter(|_| self.screenshot_owner.as_deref() == session)?;
        Some(ScreenshotContext {
            snapshot_id: self.id,
            window_id: self.window_id,
            scale,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenshotContext {
    pub snapshot_id: u32,
    pub window_id: u64,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomContext {
    pub screenshot: ScreenshotContext,
    pub origin_x: f64,
    pub origin_y: f64,
    pub scale_inv: f64,
}

impl ZoomContext {
    pub fn zoom_to_window(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin_x + x * self.scale_inv,
            self.origin_y + y * self.scale_inv,
        )
    }
}

fn screenshot_context_refusal(pid: Option<i32>, window_id: Option<u64>) -> ToolResult {
    ToolResult::error(
        "No current snapshot for this window contains a screenshot owned by this session. Call get_window_state with a screenshot on the same connection before using pixels.",
    )
    .with_structured(serde_json::json!({
        "code": "screenshot_context_missing",
        "pid": pid,
        "window_id": window_id,
    }))
}

fn stale_token_refusal<S>(pid: i32, lane: &[Snapshot<S>]) -> ToolResult {
    let current: Vec<_> = lane
        .iter()
        .map(|snapshot| (format_snapshot_id(snapshot.id), snapshot.window_id))
        .collect();
    let message = match current.as_slice() {
        [] => format!("{STALE_TOKEN_ERROR}; pid {pid} has no current snapshot"),
        current => format!(
            "{STALE_TOKEN_ERROR}; current snapshots for pid {pid}: {}",
            current
                .iter()
                .map(|(snapshot_id, window_id)| format!("{snapshot_id} (window {window_id})"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": { "code": "stale_element_token", "message": message },
        "current_snapshots": current
            .iter()
            .map(|(snapshot_id, window_id)| {
                serde_json::json!({ "snapshot_id": snapshot_id, "window_id": window_id })
            })
            .collect::<Vec<_>>(),
    }))
}

fn zoom_context_refusal(pid: i32, window_id: Option<u64>) -> ToolResult {
    ToolResult::error(
        "The zoom coordinate context is missing or was replaced by a newer snapshot. Call get_window_state and zoom again on the same connection before using from_zoom coordinates.",
    )
    .with_structured(serde_json::json!({
        "code": "zoom_context_missing",
        "pid": pid,
        "window_id": window_id,
    }))
}

pub struct SnapshotStore<S: SnapshotPayload> {
    runtime_scope: String,
    inner: Mutex<HashMap<i32, Vec<Snapshot<S>>>>,
}

impl<S: SnapshotPayload> SnapshotStore<S> {
    pub fn new() -> Self {
        Self {
            runtime_scope: current_runtime_scope(),
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn publish(&self, pid: i32, window_id: u64, payload: S) -> u32 {
        self.publish_for_session(pid, window_id, payload, None, None)
            .expect("anonymous snapshot publication cannot be retired")
            .0
    }

    /// Publish the latest runtime-owned snapshot and its screenshot coordinate frame,
    /// returning its id and the ids of the snapshots it replaced or evicted.
    ///
    /// `screenshot_scale` is the native-image-width / delivered-image-width ratio.
    /// A `None` scale deliberately records that the latest observation did not
    /// deliver an actionable screenshot. Publications completing after their
    /// owning session ended are discarded instead of resurrecting retired state.
    pub fn publish_for_session(
        &self,
        pid: i32,
        window_id: u64,
        payload: S,
        session: Option<&str>,
        screenshot_scale: Option<f64>,
    ) -> Option<(u32, Vec<u32>)> {
        let (id, retired) = {
            let mut inner = self.inner.lock().unwrap();
            if session.is_some_and(crate::session::is_session_ended) {
                return None;
            }
            let lane = inner.entry(pid).or_default();
            let mut retired = Vec::new();
            if let Some(position) = lane.iter().position(|entry| entry.window_id == window_id) {
                retired.push(lane.remove(position));
            }
            if lane.len() == LRU_CAP_PER_PID {
                retired.push(lane.remove(0));
            }
            let id = crate::element_token::mint_snapshot_id();
            lane.push(Snapshot {
                id,
                window_id,
                screenshot_owner: session.map(str::to_owned),
                screenshot_scale,
                zoom: None,
                payload,
            });
            (id, retired)
        };
        let invalidated = retired.iter().map(|snapshot| snapshot.id).collect();
        drop(retired);
        Some((id, invalidated))
    }

    /// Resolve the screenshot transform from the same authoritative latest
    /// snapshot used for element tokens. Without a window, every snapshot of
    /// the process must agree on one transform owned by this session.
    pub fn screenshot_context(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<ScreenshotContext, ToolResult> {
        let inner = self.inner.lock().unwrap();
        let lane = inner.get(&pid).map(Vec::as_slice).unwrap_or_default();
        let context = match window_id {
            Some(window_id) => lane
                .iter()
                .find(|snapshot| snapshot.window_id == window_id)
                .and_then(|snapshot| snapshot.screenshot(session)),
            None => {
                let mut contexts = lane.iter().map(|snapshot| snapshot.screenshot(session));
                contexts.next().flatten().filter(|first| {
                    contexts.all(|context| {
                        context.is_some_and(|context| (context.scale - first.scale).abs() < 1e-9)
                    })
                })
            }
        };
        context.ok_or_else(|| screenshot_context_refusal(Some(pid), window_id))
    }

    pub fn screenshot_context_for_zoom(
        &self,
        pid: Option<i32>,
        window_id: u64,
        session: Option<&str>,
    ) -> Result<(i32, ScreenshotContext), ToolResult> {
        if let Some(pid) = pid {
            return self
                .screenshot_context(pid, Some(window_id), session)
                .map(|context| (pid, context));
        }
        let inner = self.inner.lock().unwrap();
        let mut matches = inner.iter().filter_map(|(pid, lane)| {
            let snapshot = lane
                .iter()
                .find(|snapshot| snapshot.window_id == window_id)?;
            Some((*pid, snapshot.screenshot(session)?))
        });
        match (matches.next(), matches.next()) {
            (Some(found), None) => Ok(found),
            _ => Err(screenshot_context_refusal(None, Some(window_id))),
        }
    }

    pub fn set_zoom(
        &self,
        pid: i32,
        session: Option<&str>,
        zoom: ZoomContext,
    ) -> Result<(), ToolResult> {
        let mut inner = self.inner.lock().unwrap();
        let snapshot = inner
            .get_mut(&pid)
            .and_then(|lane| {
                lane.iter_mut()
                    .find(|snapshot| snapshot.window_id == zoom.screenshot.window_id)
            })
            .filter(|snapshot| snapshot.screenshot(session) == Some(zoom.screenshot))
            .ok_or_else(|| zoom_context_refusal(pid, Some(zoom.screenshot.window_id)))?;
        snapshot.zoom = Some(zoom);
        Ok(())
    }

    pub fn zoom(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<ZoomContext, ToolResult> {
        let inner = self.inner.lock().unwrap();
        let mut zooms = inner
            .get(&pid)
            .into_iter()
            .flatten()
            .filter(|snapshot| {
                window_id.is_none_or(|window_id| snapshot.window_id == window_id)
                    && snapshot.screenshot_owner.as_deref() == session
            })
            .filter_map(|snapshot| snapshot.zoom);
        match (zooms.next(), zooms.next()) {
            (Some(zoom), None) => Ok(zoom),
            _ => Err(zoom_context_refusal(pid, window_id)),
        }
    }

    pub fn retire_session_screenshots(&self, session: &str) -> usize {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            let mut retired = Vec::new();
            for lane in inner.values_mut() {
                let (owned, kept) = std::mem::take(lane)
                    .into_iter()
                    .partition(|snapshot| snapshot.screenshot_owner.as_deref() == Some(session));
                *lane = kept;
                retired.extend::<Vec<_>>(owned);
            }
            inner.retain(|_, lane| !lane.is_empty());
            retired
        };
        let count = retired.len();
        drop(retired);
        count
    }

    pub fn resolve(
        &self,
        pid: i32,
        args: &serde_json::Value,
    ) -> Result<ResolvedElement<S::Element>, ToolResult> {
        let Some(token) = args
            .get("element_token")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(ResolvedElement::None);
        };
        let (snapshot_id, element_index) = parse_token(token).ok_or_else(|| {
            refusal(
                "invalid_element_token",
                "element_token has invalid format".into(),
            )
        })?;
        let inner = self.inner.lock().unwrap();
        let lane = inner.get(&pid).map(Vec::as_slice).unwrap_or_default();
        let Some(snapshot) = lane.iter().find(|snapshot| snapshot.id == snapshot_id) else {
            return Err(stale_token_refusal(pid, lane));
        };
        let element = snapshot.payload.retain(element_index).ok_or_else(|| {
            refusal(
                "invalid_element_token",
                format!(
                    "element_token element_index {element_index} out of range (snapshot had {} elements)",
                    snapshot.payload.len()
                ),
            )
        })?;
        Ok(ResolvedElement::Element {
            window_id: snapshot.window_id,
            element_index,
            element,
        })
    }

    pub fn remove(&self, pid: i32, window_id: u64) -> Option<u32> {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            inner.get_mut(&pid).and_then(|lane| {
                let position = lane.iter().position(|entry| entry.window_id == window_id)?;
                Some(lane.remove(position))
            })
        };
        retired.map(|snapshot| snapshot.id)
    }

    pub fn clear(&self) -> usize {
        let retired = std::mem::take(&mut *self.inner.lock().unwrap());
        let count = retired.len();
        drop(retired);
        count
    }
}

impl<S: SnapshotPayload> Drop for SnapshotStore<S> {
    fn drop(&mut self) {
        let mut caches = runtime_stores().lock().unwrap();
        if caches
            .get(&self.runtime_scope)
            .is_some_and(|cache| std::ptr::addr_eq(cache.as_ptr(), self as *const Self))
        {
            caches.remove(&self.runtime_scope);
        }
        if caches.is_empty() {
            caches.shrink_to_fit();
        }
    }
}

impl<S: SnapshotPayload> Default for SnapshotStore<S> {
    fn default() -> Self {
        Self::new()
    }
}

trait RuntimeStore: Any + Send + Sync {
    fn clear(&self) -> usize;
}

impl<S: SnapshotPayload> RuntimeStore for SnapshotStore<S> {
    fn clear(&self) -> usize {
        self.clear()
    }
}

fn runtime_stores() -> &'static Mutex<HashMap<String, Weak<dyn RuntimeStore>>> {
    static STORES: OnceLock<Mutex<HashMap<String, Weak<dyn RuntimeStore>>>> = OnceLock::new();
    STORES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current_runtime_scope() -> String {
    crate::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into())
}

pub fn register_runtime_store<S: SnapshotPayload>(cache: &Arc<SnapshotStore<S>>) {
    let erased: Arc<dyn RuntimeStore> = cache.clone();
    let mut caches = runtime_stores().lock().unwrap();
    caches.retain(|_, cache| cache.strong_count() > 0);
    caches.insert(cache.runtime_scope.clone(), Arc::downgrade(&erased));
}

pub fn current_runtime_store<S: SnapshotPayload>() -> Option<Arc<SnapshotStore<S>>> {
    let cache = runtime_stores()
        .lock()
        .unwrap()
        .get(&current_runtime_scope())?
        .upgrade()?;
    let erased: Arc<dyn Any + Send + Sync> = cache;
    erased.downcast().ok()
}

pub fn retire_runtime_scope(runtime_scope: &str) -> usize {
    let cache = runtime_stores()
        .lock()
        .unwrap()
        .remove(runtime_scope)
        .and_then(|cache| cache.upgrade());
    cache.map_or(0, |cache| cache.clear())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element_token::token_for;
    use crate::snapshot_test_support::Payload;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn publish_then_resolve_returns_projection() {
        let cache = SnapshotStore::new();
        let id = cache.publish(42, 7, Payload(vec![10, 20, 30]));
        let result = cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(id, 2) }),
            )
            .unwrap();
        assert!(matches!(
            result,
            ResolvedElement::Element { element: 30, .. }
        ));
    }

    #[test]
    fn miss_returns_refusal() {
        let cache = SnapshotStore::<Payload>::new();
        assert!(cache
            .resolve(1, &serde_json::json!({ "element_token": token_for(0, 0) }))
            .is_err());
    }

    #[test]
    fn membership_matches_payload_length() {
        let cache = SnapshotStore::new();
        let id = cache.publish(9, 99, Payload(vec![1, 2, 3, 4, 5]));
        for index in 0..5 {
            assert!(cache
                .resolve(
                    9,
                    &serde_json::json!({ "element_token": token_for(id, index) })
                )
                .is_ok());
        }
        assert!(cache
            .resolve(9, &serde_json::json!({ "element_token": token_for(id, 5) }))
            .is_err());
    }

    fn refusal_code(result: ToolResult) -> String {
        result.structured_content.unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn scale(cache: &SnapshotStore<Payload>, window_id: u64, session: &str) -> Option<f64> {
        cache
            .screenshot_context(10, Some(window_id), Some(session))
            .ok()
            .map(|context| context.scale)
    }

    fn zoom_on(snapshot: u32, scale: f64) -> ZoomContext {
        ZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot,
                window_id: 20,
                scale,
            },
            origin_x: 100.0,
            origin_y: 50.0,
            scale_inv: 2.0,
        }
    }

    #[test]
    fn screenshot_coordinates_never_borrow_another_sessions_latest_transform() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        assert_eq!(scale(&cache, 20, "client-a"), Some(7.35));

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0));
        assert_eq!(scale(&cache, 20, "client-b"), Some(1.0));
        let refusal = cache
            .screenshot_context(10, Some(20), Some("client-a"))
            .expect_err("stale image coordinates must be refused");
        assert_eq!(refusal_code(refusal), "screenshot_context_missing");
    }

    #[test]
    fn screenshot_transforms_are_independent_across_windows() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("client-b"), Some(2.0));
        assert_eq!(scale(&cache, 20, "client-a"), Some(7.35));
        assert_eq!(scale(&cache, 21, "client-b"), Some(2.0));
    }

    #[test]
    fn same_session_latest_snapshot_replaces_or_refuses_older_image_context() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert_eq!(
            scale(&cache, 20, "client-a"),
            Some(1.0),
            "a newer native capture replaces the older resized frame"
        );

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), None);
        assert_eq!(
            scale(&cache, 20, "client-a"),
            None,
            "a newer tree-only observation retires the older image frame"
        );
    }

    #[test]
    fn window_relative_pixels_require_a_current_snapshot() {
        let cache = SnapshotStore::<Payload>::new();
        assert_eq!(scale(&cache, 20, "client-a"), None);
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0));
        cache.remove(10, 20);
        assert_eq!(scale(&cache, 20, "client-a"), None);
    }

    #[test]
    fn window_less_screenshot_context_requires_one_agreed_transform() {
        let cache = SnapshotStore::new();
        assert!(cache
            .screenshot_context(10, None, Some("client-a"))
            .is_err());
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("client-a"), Some(2.0));
        assert_eq!(
            cache
                .screenshot_context(10, None, Some("client-a"))
                .unwrap()
                .scale,
            2.0
        );
        cache.publish_for_session(10, 22, Payload(vec![]), Some("client-a"), Some(3.0));
        assert!(cache
            .screenshot_context(10, None, Some("client-a"))
            .is_err());
    }

    #[test]
    fn publication_reports_replaced_and_evicted_snapshots() {
        let cache = SnapshotStore::new();
        let (first, invalidated) = cache
            .publish_for_session(10, 0, Payload(vec![]), None, None)
            .unwrap();
        assert!(invalidated.is_empty());
        let (second, invalidated) = cache
            .publish_for_session(10, 0, Payload(vec![]), None, None)
            .unwrap();
        assert_eq!(invalidated, vec![first]);
        for window_id in 1..LRU_CAP_PER_PID as u64 {
            cache.publish(10, window_id, Payload(vec![]));
        }
        let (_, invalidated) = cache
            .publish_for_session(10, LRU_CAP_PER_PID as u64, Payload(vec![]), None, None)
            .unwrap();
        assert_eq!(invalidated, vec![second]);
    }

    #[test]
    fn session_retirement_removes_only_snapshots_owned_by_that_session() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("ending"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("survivor"), Some(2.0));
        assert_eq!(cache.retire_session_screenshots("ending"), 1);
        assert_eq!(scale(&cache, 20, "ending"), None);
        assert_eq!(scale(&cache, 21, "survivor"), Some(2.0));
    }

    #[test]
    fn capture_completing_after_session_end_is_not_published() {
        let cache = SnapshotStore::new();
        let session = format!("snapshot-late-capture-{}", uuid::Uuid::new_v4());
        assert!(crate::session::fire_session_end(&session));
        assert_eq!(
            cache.publish_for_session(10, 20, Payload(vec![]), Some(&session), Some(7.35)),
            None
        );
        assert_eq!(scale(&cache, 20, &session), None);
    }

    #[test]
    fn zoom_context_is_bound_to_snapshot_session_and_window() {
        let cache = SnapshotStore::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap()
            .0;
        let context = zoom_on(snapshot, 7.35);
        cache.set_zoom(10, Some("client-a"), context).unwrap();

        assert_eq!(cache.zoom(10, Some(20), Some("client-a")).unwrap(), context);
        assert_eq!(cache.zoom(10, None, Some("client-a")).unwrap(), context);
        assert_eq!(context.zoom_to_window(3.0, 4.0), (106.0, 58.0));
        for (window_id, session) in [(20, "client-b"), (21, "client-a")] {
            assert_eq!(
                refusal_code(cache.zoom(10, Some(window_id), Some(session)).unwrap_err()),
                "zoom_context_missing"
            );
        }
    }

    #[test]
    fn window_only_screenshot_lookup_requires_one_current_owned_snapshot() {
        let cache = SnapshotStore::new();
        let first = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap()
            .0;
        assert_eq!(
            cache
                .screenshot_context_for_zoom(None, 20, Some("client-a"))
                .unwrap(),
            (
                10,
                ScreenshotContext {
                    snapshot_id: first,
                    window_id: 20,
                    scale: 2.0,
                }
            )
        );
        assert_eq!(
            cache
                .screenshot_context_for_zoom(Some(10), 20, Some("client-a"))
                .unwrap()
                .0,
            10
        );
        assert!(cache
            .screenshot_context_for_zoom(None, 20, Some("client-b"))
            .is_err());

        cache.publish_for_session(11, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert!(cache
            .screenshot_context_for_zoom(None, 20, Some("client-a"))
            .is_err());
    }

    #[test]
    fn late_zoom_completion_cannot_replace_newer_valid_context() {
        let cache = SnapshotStore::new();
        let snapshot_a = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap()
            .0;
        let snapshot_b = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap()
            .0;
        let valid_b = zoom_on(snapshot_b, 1.0);
        cache.set_zoom(10, Some("client-a"), valid_b).unwrap();
        assert!(cache
            .set_zoom(10, Some("client-a"), zoom_on(snapshot_a, 2.0))
            .is_err());
        assert_eq!(cache.zoom(10, Some(20), Some("client-a")).unwrap(), valid_b);
    }

    #[test]
    fn newer_snapshot_or_session_end_retires_zoom() {
        let cache = SnapshotStore::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap()
            .0;
        cache
            .set_zoom(10, Some("client-a"), zoom_on(snapshot, 7.35))
            .unwrap();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0));
        assert_eq!(
            refusal_code(cache.zoom(10, Some(20), Some("client-a")).unwrap_err()),
            "zoom_context_missing"
        );

        let latest = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap()
            .0;
        cache
            .set_zoom(10, Some("client-a"), zoom_on(latest, 1.0))
            .unwrap();
        assert_eq!(cache.retire_session_screenshots("client-a"), 1);
        assert!(cache.zoom(10, Some(20), Some("client-a")).is_err());
    }

    struct DropCounter {
        owner: Weak<SnapshotStore<DropCounter>>,
        drops: Arc<AtomicUsize>,
    }
    impl SnapshotPayload for DropCounter {
        type Element = ();
        fn len(&self) -> usize {
            1
        }
        fn retain(&self, index: usize) -> Option<()> {
            (index == 0).then_some(())
        }
    }
    impl Drop for DropCounter {
        fn drop(&mut self) {
            if let Some(owner) = self.owner.upgrade() {
                assert!(
                    owner.inner.try_lock().is_ok(),
                    "native cleanup ran under the storage lock"
                );
            }
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn session_retirement_drops_payload_outside_lock() {
        let cache = Arc::new(SnapshotStore::new());
        let drops = Arc::new(AtomicUsize::new(0));
        cache.publish_for_session(
            10,
            20,
            DropCounter {
                owner: Arc::downgrade(&cache),
                drops: drops.clone(),
            },
            Some("ending"),
            Some(2.0),
        );
        assert_eq!(cache.retire_session_screenshots("ending"), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(cache.inner.lock().unwrap().is_empty());
    }

    #[test]
    fn replacement_remove_and_clear_run_drop_outside_lock() {
        let cache = Arc::new(SnapshotStore::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = || DropCounter {
            owner: Arc::downgrade(&cache),
            drops: drops.clone(),
        };
        cache.publish(1, 1, payload());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        cache.publish(1, 1, payload());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        cache.remove(1, 1);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        cache.publish(1, 1, payload());
        cache.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn window_identity_preserves_high_bits_through_resolution_and_retirement() {
        let cache = SnapshotStore::new();
        let low = 7;
        let high = (1_u64 << 32) | low;
        let first = cache.publish(42, low, Payload(vec![10]));
        let second = cache.publish(42, high, Payload(vec![20]));
        let token = serde_json::json!({ "element_token": token_for(second, 0) });
        assert!(matches!(
            cache.resolve(42, &token).unwrap(),
            ResolvedElement::Element { window_id, element: 20, .. } if window_id == high
        ));
        cache.remove(42, high);
        assert!(cache.resolve(42, &token).is_err());
        assert!(matches!(
            cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(first, 0) })
                )
                .unwrap(),
            ResolvedElement::Element { window_id: 7, .. }
        ));
    }

    #[test]
    fn bindings_sharing_a_scope_keep_independent_payload_ownership() {
        crate::tool::with_runtime_scope("snapshot-binding-ownership".into(), || {
            let first = Arc::new(SnapshotStore::new());
            let second = Arc::new(SnapshotStore::new());
            register_runtime_store(&first);
            register_runtime_store(&second);
            let first_id = first.publish(42, 7, Payload(vec![10]));
            let second_id = second.publish(42, 7, Payload(vec![20]));
            assert!(second
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(first_id, 0) })
                )
                .is_err());
            drop(first);
            let resolved = second
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(second_id, 0) }),
                )
                .unwrap();
            assert!(matches!(
                resolved,
                ResolvedElement::Element { element: 20, .. }
            ));
            assert!(Arc::ptr_eq(
                &current_runtime_store::<Payload>().unwrap(),
                &second
            ));
            retire_runtime_scope("snapshot-binding-ownership");
        });
    }

    #[test]
    fn recording_discovery_does_not_extend_payload_lifetime() {
        crate::tool::with_runtime_scope("snapshot-weak-discovery".into(), || {
            let cache = Arc::new(SnapshotStore::new());
            let drops = Arc::new(AtomicUsize::new(0));
            cache.publish(
                42,
                7,
                DropCounter {
                    owner: Arc::downgrade(&cache),
                    drops: drops.clone(),
                },
            );
            register_runtime_store(&cache);
            assert_eq!(Arc::strong_count(&cache), 1);
            drop(cache);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert!(current_runtime_store::<DropCounter>().is_none());
            {
                let caches = runtime_stores().lock().unwrap();
                assert!(!caches.contains_key("snapshot-weak-discovery"));
                if caches.is_empty() {
                    assert_eq!(caches.capacity(), 0);
                }
            }
            assert_eq!(retire_runtime_scope("snapshot-weak-discovery"), 0);
        });
    }

    #[test]
    fn default_impl_matches_new() {
        let _cache: SnapshotStore<Payload> = SnapshotStore::default();
    }
}
