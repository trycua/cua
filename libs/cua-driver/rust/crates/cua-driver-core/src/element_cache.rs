use crate::element_token::{
    parse_element_args, refusal, ResolvedElement, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
};
use crate::protocol::ToolResult;
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
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
    payload: S,
}

struct ElementCacheState<S> {
    snapshots: HashMap<i32, Vec<Snapshot<S>>>,
    retired_screenshots: HashSet<(i32, u64)>,
    retired_screenshot_order: VecDeque<(i32, u64)>,
    retired_screenshot_overflowed: bool,
}

const RETIRED_SCREENSHOT_CAPACITY: usize = 256;

impl<S> Default for ElementCacheState<S> {
    fn default() -> Self {
        Self {
            snapshots: HashMap::new(),
            retired_screenshots: HashSet::new(),
            retired_screenshot_order: VecDeque::new(),
            retired_screenshot_overflowed: false,
        }
    }
}

impl<S> ElementCacheState<S> {
    fn retire_screenshot(&mut self, key: (i32, u64)) {
        if self.retired_screenshot_overflowed || self.retired_screenshots.contains(&key) {
            return;
        }
        if self.retired_screenshots.len() == RETIRED_SCREENSHOT_CAPACITY {
            self.retired_screenshot_overflowed = true;
            return;
        }
        self.retired_screenshots.insert(key);
        self.retired_screenshot_order.push_back(key);
    }

    fn restore_screenshot(&mut self, key: (i32, u64)) {
        if self.retired_screenshots.remove(&key) {
            self.retired_screenshot_order.retain(|entry| *entry != key);
        }
    }

    fn missing_screenshot_is_retired(&self, pid: i32, window_id: Option<u64>) -> bool {
        self.retired_screenshot_overflowed
            || match window_id {
                Some(window_id) => self.retired_screenshots.contains(&(pid, window_id)),
                None => self
                    .retired_screenshots
                    .iter()
                    .any(|(retired_pid, _)| *retired_pid == pid),
            }
    }

    fn reset_retired_screenshots(&mut self) {
        self.retired_screenshots.clear();
        self.retired_screenshot_order.clear();
        self.retired_screenshot_overflowed = false;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenshotContextError {
    ReplacedOrUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenshotContext {
    pub snapshot_id: u32,
    pub window_id: u64,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotBoundZoomContext {
    pub screenshot: ScreenshotContext,
    pub origin_x: f64,
    pub origin_y: f64,
    pub scale_inv: f64,
}

impl SnapshotBoundZoomContext {
    pub fn zoom_to_window(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin_x + x * self.scale_inv,
            self.origin_y + y * self.scale_inv,
        )
    }
}

pub struct SnapshotBoundZoomRegistry {
    inner: Mutex<HashMap<(i32, u64, Option<String>), SnapshotBoundZoomContext>>,
}

impl SnapshotBoundZoomRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_if_current<S: SnapshotPayload>(
        &self,
        cache: &ElementCacheCore<S>,
        pid: i32,
        session: Option<&str>,
        context: SnapshotBoundZoomContext,
    ) -> Result<(), ToolResult> {
        cache
            .with_valid_screenshot_context(pid, session, context.screenshot, || {
                self.inner.lock().unwrap().insert(
                    (
                        pid,
                        context.screenshot.window_id,
                        session.map(str::to_owned),
                    ),
                    context,
                );
            })
            .map_err(|_| zoom_context_refusal(pid, Some(context.screenshot.window_id)))
    }

    pub fn resolve<S: SnapshotPayload>(
        &self,
        cache: &ElementCacheCore<S>,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<SnapshotBoundZoomContext, ToolResult> {
        let owner = session.map(str::to_owned);
        let (key, context) = {
            let inner = self.inner.lock().unwrap();
            let mut matches = inner
                .iter()
                .filter(|((entry_pid, entry_window, entry_owner), _)| {
                    *entry_pid == pid
                        && window_id.is_none_or(|window_id| *entry_window == window_id)
                        && entry_owner == &owner
                });
            let Some((key, context)) = matches.next() else {
                return Err(zoom_context_refusal(pid, window_id));
            };
            if matches.next().is_some() {
                return Err(zoom_context_refusal(pid, window_id));
            }
            (key.clone(), *context)
        };
        if cache
            .validate_screenshot_context(pid, session, context.screenshot)
            .is_err()
        {
            self.inner.lock().unwrap().remove(&key);
            return Err(zoom_context_refusal(pid, window_id));
        }
        Ok(context)
    }

    pub fn retire_replaced(&self, pid: i32, window_id: u64, current_snapshot_id: u32) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.len();
        inner.retain(|(entry_pid, entry_window, _), context| {
            *entry_pid != pid
                || *entry_window != window_id
                || context.screenshot.snapshot_id == current_snapshot_id
        });
        inner.len() != before
    }

    pub fn retire_session(&self, session: &str) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.len();
        inner.retain(|(_, _, owner), _| owner.as_deref() != Some(session));
        before - inner.len()
    }
}

impl Default for SnapshotBoundZoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn screenshot_context_refusal(pid: Option<i32>, window_id: Option<u64>) -> ToolResult {
    ToolResult::error(
        "The latest snapshot for this window does not contain a screenshot owned by this session. Call get_window_state with a screenshot on the same connection before using pixels.",
    )
    .with_structured(serde_json::json!({
        "code": "screenshot_context_missing",
        "pid": pid,
        "window_id": window_id,
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

pub struct ElementCacheCore<S: SnapshotPayload> {
    runtime_scope: String,
    inner: Mutex<ElementCacheState<S>>,
}

impl<S: SnapshotPayload> ElementCacheCore<S> {
    pub fn new() -> Self {
        Self {
            runtime_scope: current_runtime_scope(),
            inner: Mutex::new(ElementCacheState::default()),
        }
    }

    pub fn publish(&self, pid: i32, window_id: u64, payload: S) -> u32 {
        self.publish_for_session(pid, window_id, payload, None, None)
            .expect("anonymous snapshot publication cannot be retired")
    }

    /// Publish the latest runtime-owned snapshot and its screenshot coordinate frame.
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
    ) -> Option<u32> {
        let (id, replaced, evicted) = {
            let mut inner = self.inner.lock().unwrap();
            if session.is_some_and(crate::session::is_session_ended) {
                return None;
            }
            inner.restore_screenshot((pid, window_id));
            let (id, replaced, evicted) = {
                let lane = inner.snapshots.entry(pid).or_default();
                let replaced = lane
                    .iter()
                    .position(|entry| entry.window_id == window_id)
                    .map(|position| lane.remove(position));
                let evicted = (lane.len() == LRU_CAP_PER_PID).then(|| lane.remove(0));
                let id = crate::element_token::mint_snapshot_id();
                lane.push(Snapshot {
                    id,
                    window_id,
                    screenshot_owner: session.map(str::to_owned),
                    screenshot_scale,
                    payload,
                });
                (id, replaced, evicted)
            };
            if let Some(evicted) = &evicted {
                inner.retire_screenshot((pid, evicted.window_id));
            }
            (id, replaced, evicted)
        };
        drop((replaced, evicted));
        Some(id)
    }

    /// Resolve the screenshot transform from the same authoritative latest
    /// snapshot used for element tokens.
    ///
    /// No snapshot preserves the legacy native-pixel fallback. Once a snapshot
    /// exists, a different owner or a newer observation without a screenshot
    /// makes older image coordinates stale and must be refused.
    pub fn screenshot_scale(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<Option<f64>, ScreenshotContextError> {
        Ok(self
            .screenshot_context(pid, window_id, session)?
            .map(|context| context.scale))
    }

    pub fn screenshot_context(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<Option<ScreenshotContext>, ScreenshotContextError> {
        let inner = self.inner.lock().unwrap();
        let Some(lane) = inner.snapshots.get(&pid) else {
            return if inner.missing_screenshot_is_retired(pid, window_id) {
                Err(ScreenshotContextError::ReplacedOrUnavailable)
            } else {
                Ok(None)
            };
        };
        if let Some(window_id) = window_id {
            let Some(snapshot) = lane.iter().find(|entry| entry.window_id == window_id) else {
                return if inner.missing_screenshot_is_retired(pid, Some(window_id)) {
                    Err(ScreenshotContextError::ReplacedOrUnavailable)
                } else {
                    Ok(None)
                };
            };
            if snapshot.screenshot_owner.as_deref() != session {
                return Err(ScreenshotContextError::ReplacedOrUnavailable);
            }
            let scale = snapshot
                .screenshot_scale
                .ok_or(ScreenshotContextError::ReplacedOrUnavailable)?;
            return Ok(Some(ScreenshotContext {
                snapshot_id: snapshot.id,
                window_id,
                scale,
            }));
        }
        if inner.missing_screenshot_is_retired(pid, None) {
            return Err(ScreenshotContextError::ReplacedOrUnavailable);
        }
        let mut agreed: Option<ScreenshotContext> = None;
        for snapshot in lane {
            if snapshot.screenshot_owner.as_deref() != session {
                return Err(ScreenshotContextError::ReplacedOrUnavailable);
            }
            let scale = snapshot
                .screenshot_scale
                .ok_or(ScreenshotContextError::ReplacedOrUnavailable)?;
            let context = ScreenshotContext {
                snapshot_id: snapshot.id,
                window_id: snapshot.window_id,
                scale,
            };
            match agreed {
                None => agreed = Some(context),
                Some(previous) if (previous.scale - scale).abs() < 1e-9 => {}
                Some(_) => return Err(ScreenshotContextError::ReplacedOrUnavailable),
            }
        }
        Ok(agreed)
    }

    pub fn validate_screenshot_context(
        &self,
        pid: i32,
        session: Option<&str>,
        expected: ScreenshotContext,
    ) -> Result<(), ScreenshotContextError> {
        match self.screenshot_context(pid, Some(expected.window_id), session)? {
            Some(current)
                if current.snapshot_id == expected.snapshot_id
                    && (current.scale - expected.scale).abs() < 1e-9 =>
            {
                Ok(())
            }
            _ => Err(ScreenshotContextError::ReplacedOrUnavailable),
        }
    }

    fn with_valid_screenshot_context<R>(
        &self,
        pid: i32,
        session: Option<&str>,
        expected: ScreenshotContext,
        publish: impl FnOnce() -> R,
    ) -> Result<R, ScreenshotContextError> {
        let inner = self.inner.lock().unwrap();
        let valid = inner.snapshots.get(&pid).is_some_and(|lane| {
            lane.iter().any(|snapshot| {
                snapshot.window_id == expected.window_id
                    && snapshot.id == expected.snapshot_id
                    && snapshot.screenshot_owner.as_deref() == session
                    && snapshot
                        .screenshot_scale
                        .is_some_and(|scale| (scale - expected.scale).abs() < 1e-9)
            })
        });
        if !valid {
            return Err(ScreenshotContextError::ReplacedOrUnavailable);
        }
        let result = publish();
        drop(inner);
        Ok(result)
    }

    pub fn unique_screenshot_context_for_window(
        &self,
        window_id: u64,
        session: Option<&str>,
    ) -> Result<(i32, ScreenshotContext), ScreenshotContextError> {
        let inner = self.inner.lock().unwrap();
        let mut matches = inner.snapshots.iter().filter_map(|(pid, lane)| {
            let snapshot = lane
                .iter()
                .find(|snapshot| snapshot.window_id == window_id)?;
            (snapshot.screenshot_owner.as_deref() == session).then_some((pid, snapshot))
        });
        let Some((pid, snapshot)) = matches.next() else {
            return Err(ScreenshotContextError::ReplacedOrUnavailable);
        };
        if matches.next().is_some() {
            return Err(ScreenshotContextError::ReplacedOrUnavailable);
        }
        let scale = snapshot
            .screenshot_scale
            .ok_or(ScreenshotContextError::ReplacedOrUnavailable)?;
        Ok((
            *pid,
            ScreenshotContext {
                snapshot_id: snapshot.id,
                window_id,
                scale,
            },
        ))
    }

    pub fn screenshot_context_or_refusal(
        &self,
        pid: i32,
        window_id: u64,
        session: Option<&str>,
    ) -> Result<ScreenshotContext, ToolResult> {
        match self.screenshot_context(pid, Some(window_id), session) {
            Ok(Some(context)) => Ok(context),
            Ok(None) | Err(ScreenshotContextError::ReplacedOrUnavailable) => {
                Err(screenshot_context_refusal(Some(pid), Some(window_id)))
            }
        }
    }

    pub fn screenshot_context_for_zoom(
        &self,
        pid: Option<i32>,
        window_id: u64,
        session: Option<&str>,
    ) -> Result<(i32, ScreenshotContext), ToolResult> {
        match pid {
            Some(pid) => self
                .screenshot_context_or_refusal(pid, window_id, session)
                .map(|context| (pid, context)),
            None => self
                .unique_screenshot_context_for_window(window_id, session)
                .map_err(|_| screenshot_context_refusal(None, Some(window_id))),
        }
    }

    pub fn screenshot_scale_or_refusal(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<f64, ToolResult> {
        match self.screenshot_scale(pid, window_id, session) {
            Ok(scale) => Ok(scale.unwrap_or(1.0)),
            Err(ScreenshotContextError::ReplacedOrUnavailable) => {
                Err(screenshot_context_refusal(Some(pid), window_id))
            }
        }
    }

    pub fn retire_session_screenshots(&self, session: &str) -> usize {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            let mut retired = Vec::new();
            let mut tombstones = Vec::new();
            for (pid, lane) in inner.snapshots.iter_mut() {
                let mut index = 0;
                while index < lane.len() {
                    if lane[index].screenshot_owner.as_deref() == Some(session) {
                        tombstones.push((*pid, lane[index].window_id));
                        retired.push(lane.remove(index));
                    } else {
                        index += 1;
                    }
                }
            }
            inner.snapshots.retain(|_, lane| !lane.is_empty());
            tombstones.sort_unstable();
            for tombstone in tombstones {
                inner.retire_screenshot(tombstone);
            }
            retired
        };
        let count = retired.len();
        drop(retired);
        count
    }

    pub fn resolve_element_args(
        &self,
        pid: i32,
        element_index: Option<usize>,
        element_token: Option<&str>,
        snapshot_id: Option<&str>,
        window_id: Option<u64>,
        tool_name: &str,
    ) -> Result<ResolvedElement<S::Element>, ToolResult> {
        let Some(reference) = parse_element_args(
            element_index,
            element_token,
            snapshot_id,
            window_id,
            tool_name,
        )?
        else {
            return Ok(ResolvedElement::None);
        };
        if current_runtime_scope() != self.runtime_scope {
            return Err(refusal(
                "generation_mismatch",
                "element_token belongs to another runtime generation".into(),
            ));
        }
        let inner = self.inner.lock().unwrap();
        let entry = inner
            .snapshots
            .get(&pid)
            .and_then(|lane| lane.iter().find(|entry| entry.id == reference.snapshot_id));
        let Some(entry) = entry else {
            drop(inner);
            let caches = runtime_caches()
                .lock()
                .unwrap()
                .iter()
                .filter(|(scope, _)| *scope != &self.runtime_scope)
                .filter_map(|(_, cache)| cache.upgrade())
                .collect::<Vec<_>>();
            let foreign = caches
                .iter()
                .any(|cache| cache.contains(pid, reference.snapshot_id));
            return Err(if foreign {
                refusal(
                    "generation_mismatch",
                    "element_token belongs to another runtime generation".into(),
                )
            } else {
                refusal("stale_element_token", STALE_TOKEN_ERROR.into())
            });
        };
        let element = entry
            .payload
            .retain(reference.element_index)
            .ok_or_else(|| {
                refusal(
                    "invalid_element_token",
                    format!(
                        "element_token element_index {} out of range (snapshot had {} elements)",
                        reference.element_index,
                        entry.payload.len()
                    ),
                )
            })?;
        let window_id = entry.window_id;
        drop(inner);
        reference.validate_window(window_id, tool_name)?;
        Ok(ResolvedElement::Element {
            window_id: Some(window_id),
            element_index: reference.element_index,
            via_token: reference.via_token,
            element,
        })
    }

    pub fn remove(&self, pid: i32, window_id: u64) {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            let retired = inner.snapshots.get_mut(&pid).and_then(|lane| {
                let position = lane.iter().position(|entry| entry.window_id == window_id)?;
                Some(lane.remove(position))
            });
            if retired.is_some() {
                inner.retire_screenshot((pid, window_id));
            }
            retired
        };
        drop(retired);
    }

    pub fn clear(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let retired = std::mem::take(&mut inner.snapshots);
        inner.reset_retired_screenshots();
        let count = retired.len();
        drop(inner);
        drop(retired);
        count
    }
}

impl<S: SnapshotPayload> Drop for ElementCacheCore<S> {
    fn drop(&mut self) {
        let mut caches = runtime_caches().lock().unwrap();
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

impl<S: SnapshotPayload> Default for ElementCacheCore<S> {
    fn default() -> Self {
        Self::new()
    }
}

trait RuntimeCache: Any + Send + Sync {
    fn contains(&self, pid: i32, snapshot_id: u32) -> bool;
    fn clear(&self) -> usize;
}

impl<S: SnapshotPayload> RuntimeCache for ElementCacheCore<S> {
    fn contains(&self, pid: i32, snapshot_id: u32) -> bool {
        self.inner
            .lock()
            .unwrap()
            .snapshots
            .get(&pid)
            .is_some_and(|lane| lane.iter().any(|entry| entry.id == snapshot_id))
    }

    fn clear(&self) -> usize {
        self.clear()
    }
}

fn runtime_caches() -> &'static Mutex<HashMap<String, Weak<dyn RuntimeCache>>> {
    static CACHES: OnceLock<Mutex<HashMap<String, Weak<dyn RuntimeCache>>>> = OnceLock::new();
    CACHES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current_runtime_scope() -> String {
    crate::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into())
}

pub fn register_runtime_cache<S: SnapshotPayload>(cache: &Arc<ElementCacheCore<S>>) {
    let erased: Arc<dyn RuntimeCache> = cache.clone();
    let mut caches = runtime_caches().lock().unwrap();
    caches.retain(|_, cache| cache.strong_count() > 0);
    caches.insert(cache.runtime_scope.clone(), Arc::downgrade(&erased));
}

pub fn current_runtime_cache<S: SnapshotPayload>() -> Option<Arc<ElementCacheCore<S>>> {
    let cache = runtime_caches()
        .lock()
        .unwrap()
        .get(&current_runtime_scope())?
        .upgrade()?;
    let erased: Arc<dyn Any + Send + Sync> = cache;
    erased.downcast().ok()
}

pub fn retire_runtime_scope(runtime_scope: &str) -> usize {
    let cache = runtime_caches()
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
        let cache = ElementCacheCore::new();
        let id = cache.publish(42, 7, Payload(vec![10, 20, 30]));
        let result = cache
            .resolve_element_args(42, None, Some(&token_for(id, 2)), None, None, "click")
            .unwrap();
        assert!(matches!(
            result,
            ResolvedElement::Element { element: 30, .. }
        ));
    }

    #[test]
    fn miss_returns_refusal() {
        let cache = ElementCacheCore::<Payload>::new();
        assert!(cache
            .resolve_element_args(1, None, Some(&token_for(0, 0)), None, None, "click")
            .is_err());
    }

    #[test]
    fn membership_matches_payload_length() {
        let cache = ElementCacheCore::new();
        let id = cache.publish(9, 99, Payload(vec![1, 2, 3, 4, 5]));
        for index in 0..5 {
            assert!(cache
                .resolve_element_args(9, None, Some(&token_for(id, index)), None, None, "click")
                .is_ok());
        }
        assert!(cache
            .resolve_element_args(9, None, Some(&token_for(id, 5)), None, None, "click")
            .is_err());
    }

    #[test]
    fn screenshot_coordinates_never_borrow_another_sessions_latest_transform() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Ok(Some(7.35))
        );

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-b")),
            Ok(Some(1.0))
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
        let refusal = cache
            .screenshot_scale_or_refusal(10, Some(20), Some("client-a"))
            .expect_err("stale image coordinates must be refused");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["code"],
            "screenshot_context_missing"
        );
    }

    #[test]
    fn screenshot_transforms_are_independent_across_windows() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("client-b"), Some(2.0));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Ok(Some(7.35))
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(21), Some("client-b")),
            Ok(Some(2.0))
        );
    }

    #[test]
    fn same_session_latest_snapshot_replaces_or_refuses_older_image_context() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Ok(Some(1.0)),
            "a newer native capture replaces the older resized frame"
        );

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), None);
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Err(ScreenshotContextError::ReplacedOrUnavailable),
            "a newer tree-only observation retires the older image frame"
        );
    }

    #[test]
    fn session_retirement_removes_only_snapshots_owned_by_that_session() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("ending"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("survivor"), Some(2.0));
        assert_eq!(cache.retire_session_screenshots("ending"), 1);
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("ending")),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(21), Some("survivor")),
            Ok(Some(2.0))
        );
    }

    #[test]
    fn retired_screenshot_refuses_cross_session_and_anonymous_replay_until_republished() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("ending"), Some(7.35));
        assert_eq!(cache.retire_session_screenshots("ending"), 1);

        for session in [Some("ending"), Some("other"), None] {
            assert_eq!(
                cache.screenshot_scale(10, Some(20), session),
                Err(ScreenshotContextError::ReplacedOrUnavailable),
                "a retired screenshot must not become native-pixel fallback"
            );
        }
        assert_eq!(
            cache.screenshot_scale(10, None, Some("other")),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );

        cache.publish_for_session(10, 20, Payload(vec![]), Some("other"), Some(2.0));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("other")),
            Ok(Some(2.0)),
            "a fresh snapshot clears the lightweight retirement tombstone"
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(20), None),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );

        cache.publish_for_session(10, 20, Payload(vec![]), None, Some(1.5));
        assert_eq!(
            cache.screenshot_scale(10, Some(20), None),
            Ok(Some(1.5)),
            "a fresh anonymous snapshot also recovers the coordinate context"
        );
    }

    #[test]
    fn lru_eviction_retires_only_the_evicted_screenshot_key() {
        let cache = ElementCacheCore::new();
        for window_id in 0..LRU_CAP_PER_PID as u64 {
            cache.publish_for_session(10, window_id, Payload(vec![]), Some("client-a"), Some(2.0));
        }
        cache.publish_for_session(
            10,
            LRU_CAP_PER_PID as u64,
            Payload(vec![]),
            Some("client-a"),
            Some(2.0),
        );

        assert_eq!(
            cache.screenshot_scale(10, Some(0), Some("client-a")),
            Err(ScreenshotContextError::ReplacedOrUnavailable),
            "evicting an observed window must not restore native-pixel fallback"
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(1), Some("client-a")),
            Ok(Some(2.0))
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(9999), Some("client-a")),
            Ok(None),
            "a never-observed key keeps the legacy fallback"
        );
    }

    #[test]
    fn explicit_remove_retires_only_a_snapshot_that_existed() {
        let cache = ElementCacheCore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0));
        cache.remove(10, 20);

        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Err(ScreenshotContextError::ReplacedOrUnavailable),
            "removing an observed window must not restore native-pixel fallback"
        );
        cache.remove(10, 21);
        assert_eq!(
            cache.screenshot_scale(10, Some(21), Some("client-a")),
            Ok(None),
            "removing an absent key must not retire a never-observed window"
        );
    }

    #[test]
    fn capture_completing_after_session_end_is_not_published() {
        let cache = ElementCacheCore::new();
        let session = format!("snapshot-late-capture-{}", uuid::Uuid::new_v4());
        assert!(crate::session::fire_session_end(&session));
        assert_eq!(
            cache.publish_for_session(10, 20, Payload(vec![]), Some(&session), Some(7.35)),
            None
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some(&session)),
            Ok(None)
        );
    }

    #[test]
    fn no_snapshot_keeps_legacy_native_pixel_fallback() {
        let cache = ElementCacheCore::<Payload>::new();
        assert_eq!(
            cache.screenshot_scale(10, Some(20), Some("client-a")),
            Ok(None)
        );
    }

    #[test]
    fn retired_screenshot_index_has_deterministic_fixed_capacity() {
        let mut state = ElementCacheState::<Payload>::default();
        let expected = (0..RETIRED_SCREENSHOT_CAPACITY)
            .map(|index| (1000 + index as i32, 2000 + index as u64))
            .collect::<Vec<_>>();
        for key in &expected {
            state.retire_screenshot(*key);
        }

        assert_eq!(state.retired_screenshots.len(), RETIRED_SCREENSHOT_CAPACITY);
        assert_eq!(
            state
                .retired_screenshot_order
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            expected
        );
        assert!(!state.retired_screenshot_overflowed);

        let order_before_duplicate = state.retired_screenshot_order.clone();
        state.retire_screenshot(expected[0]);
        assert_eq!(state.retired_screenshot_order, order_before_duplicate);

        state.retire_screenshot((9999, 9999));
        assert!(state.retired_screenshot_overflowed);
        assert_eq!(state.retired_screenshots.len(), RETIRED_SCREENSHOT_CAPACITY);
        assert_eq!(state.retired_screenshot_order, order_before_duplicate);
        assert!(!state.retired_screenshots.contains(&(9999, 9999)));
    }

    fn overflow_retired_screenshots(cache: &ElementCacheCore<Payload>) -> (i32, u64) {
        for index in 0..=RETIRED_SCREENSHOT_CAPACITY {
            cache.publish_for_session(
                1000 + index as i32,
                2000 + index as u64,
                Payload(vec![]),
                Some("ending"),
                Some(2.0),
            );
        }
        assert_eq!(
            cache.retire_session_screenshots("ending"),
            RETIRED_SCREENSHOT_CAPACITY + 1
        );
        (
            1000 + RETIRED_SCREENSHOT_CAPACITY as i32,
            2000 + RETIRED_SCREENSHOT_CAPACITY as u64,
        )
    }

    #[test]
    fn retired_screenshot_overflow_refuses_unrecorded_and_unseen_replay() {
        let cache = ElementCacheCore::new();
        let overflow_key = overflow_retired_screenshots(&cache);

        let inner = cache.inner.lock().unwrap();
        assert!(inner.retired_screenshot_overflowed);
        assert_eq!(inner.retired_screenshots.len(), RETIRED_SCREENSHOT_CAPACITY);
        assert_eq!(
            inner.retired_screenshot_order.len(),
            RETIRED_SCREENSHOT_CAPACITY
        );
        assert!(!inner.retired_screenshots.contains(&overflow_key));
        drop(inner);

        assert_eq!(
            cache.screenshot_scale(overflow_key.0, Some(overflow_key.1), None),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
        assert_eq!(
            cache.screenshot_scale(9999, Some(9999), None),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
    }

    #[test]
    fn fresh_snapshot_resolves_while_retirement_index_is_overflowed() {
        let cache = ElementCacheCore::new();
        let overflow_key = overflow_retired_screenshots(&cache);

        cache.publish_for_session(
            overflow_key.0,
            overflow_key.1,
            Payload(vec![]),
            Some("fresh"),
            Some(3.0),
        );
        assert_eq!(
            cache.screenshot_scale(overflow_key.0, Some(overflow_key.1), Some("fresh")),
            Ok(Some(3.0))
        );
        assert_eq!(
            cache.screenshot_scale(overflow_key.0, Some(overflow_key.1), Some("other")),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
        assert_eq!(
            cache.screenshot_scale(9999, Some(9999), Some("fresh")),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
    }

    #[test]
    fn clear_resets_retired_screenshot_overflow() {
        let cache = ElementCacheCore::new();
        overflow_retired_screenshots(&cache);
        cache.clear();

        let inner = cache.inner.lock().unwrap();
        assert!(inner.retired_screenshots.is_empty());
        assert!(inner.retired_screenshot_order.is_empty());
        assert!(!inner.retired_screenshot_overflowed);
        drop(inner);
        assert_eq!(cache.screenshot_scale(9999, Some(9999), None), Ok(None));
    }

    #[test]
    fn zoom_context_is_bound_to_snapshot_session_and_window() {
        let cache = ElementCacheCore::new();
        let zooms = SnapshotBoundZoomRegistry::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap();
        let context = SnapshotBoundZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot,
                window_id: 20,
                scale: 7.35,
            },
            origin_x: 100.0,
            origin_y: 50.0,
            scale_inv: 2.0,
        };
        zooms
            .set_if_current(&cache, 10, Some("client-a"), context)
            .unwrap();

        assert_eq!(
            zooms
                .resolve(&cache, 10, Some(20), Some("client-a"))
                .unwrap(),
            context
        );
        assert_eq!(context.zoom_to_window(3.0, 4.0), (106.0, 58.0));
        assert_eq!(
            zooms
                .resolve(&cache, 10, Some(20), Some("client-b"))
                .unwrap_err()
                .structured_content
                .as_ref()
                .unwrap()["code"],
            "zoom_context_missing"
        );
        assert_eq!(
            zooms
                .resolve(&cache, 10, Some(21), Some("client-a"))
                .unwrap_err()
                .structured_content
                .as_ref()
                .unwrap()["code"],
            "zoom_context_missing"
        );
    }

    #[test]
    fn window_only_screenshot_lookup_requires_one_current_owned_snapshot() {
        let cache = ElementCacheCore::new();
        let first = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap();
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
            .unique_screenshot_context_for_window(20, Some("client-b"))
            .is_err());

        cache.publish_for_session(11, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert!(cache
            .unique_screenshot_context_for_window(20, Some("client-a"))
            .is_err());
    }

    #[test]
    fn late_zoom_completion_cannot_replace_newer_valid_context() {
        let cache = ElementCacheCore::new();
        let zooms = SnapshotBoundZoomRegistry::new();
        let snapshot_a = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap();
        let slow_a = SnapshotBoundZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot_a,
                window_id: 20,
                scale: 2.0,
            },
            origin_x: 10.0,
            origin_y: 20.0,
            scale_inv: 2.0,
        };

        let snapshot_b = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap();
        let valid_b = SnapshotBoundZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot_b,
                window_id: 20,
                scale: 1.0,
            },
            origin_x: 30.0,
            origin_y: 40.0,
            scale_inv: 1.0,
        };
        zooms
            .set_if_current(&cache, 10, Some("client-a"), valid_b)
            .unwrap();
        assert!(zooms
            .set_if_current(&cache, 10, Some("client-a"), slow_a)
            .is_err());
        assert_eq!(
            zooms
                .resolve(&cache, 10, Some(20), Some("client-a"))
                .unwrap(),
            valid_b
        );
    }

    #[test]
    fn newer_snapshot_retires_zoom_for_click_drag_and_held_pointer_coordinates() {
        let cache = ElementCacheCore::new();
        let zooms = SnapshotBoundZoomRegistry::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap();
        let context = SnapshotBoundZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot,
                window_id: 20,
                scale: 7.35,
            },
            origin_x: 100.0,
            origin_y: 50.0,
            scale_inv: 2.0,
        };
        zooms
            .set_if_current(&cache, 10, Some("client-a"), context)
            .unwrap();

        let click = context.zoom_to_window(1.0, 2.0);
        let drag_from = context.zoom_to_window(3.0, 4.0);
        let held_pointer_to = context.zoom_to_window(5.0, 6.0);
        assert_eq!(
            (click, drag_from, held_pointer_to),
            ((102.0, 54.0), (106.0, 58.0), (110.0, 62.0))
        );

        let replacement = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0))
            .unwrap();
        assert!(zooms.retire_replaced(10, 20, replacement));
        let refusal = zooms
            .resolve(&cache, 10, Some(20), Some("client-a"))
            .expect_err("all uses of the old zoom image must become stale together");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["code"],
            "zoom_context_missing"
        );
    }

    #[test]
    fn zoom_context_retires_on_same_session_replacement_and_session_end() {
        let cache = ElementCacheCore::new();
        let zooms = SnapshotBoundZoomRegistry::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap();
        zooms
            .set_if_current(
                &cache,
                10,
                Some("client-a"),
                SnapshotBoundZoomContext {
                    screenshot: ScreenshotContext {
                        snapshot_id: snapshot,
                        window_id: 20,
                        scale: 7.35,
                    },
                    origin_x: 0.0,
                    origin_y: 0.0,
                    scale_inv: 1.0,
                },
            )
            .unwrap();

        let replacement = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap();
        assert!(zooms.retire_replaced(10, 20, replacement));
        assert!(zooms
            .resolve(&cache, 10, Some(20), Some("client-a"))
            .is_err());

        let latest = cache
            .screenshot_context(10, Some(20), Some("client-a"))
            .unwrap()
            .unwrap();
        zooms
            .set_if_current(
                &cache,
                10,
                Some("client-a"),
                SnapshotBoundZoomContext {
                    screenshot: latest,
                    origin_x: 0.0,
                    origin_y: 0.0,
                    scale_inv: 1.0,
                },
            )
            .unwrap();
        assert_eq!(zooms.retire_session("client-a"), 1);
        assert!(zooms
            .resolve(&cache, 10, Some(20), Some("client-a"))
            .is_err());
    }

    struct DropCounter {
        owner: Weak<ElementCacheCore<DropCounter>>,
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
    fn screenshot_retirement_keeps_only_tombstone_not_payload() {
        let cache = Arc::new(ElementCacheCore::new());
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
        let inner = cache.inner.lock().unwrap();
        assert!(inner.snapshots.is_empty());
        assert_eq!(inner.retired_screenshots.len(), 1);
        assert_eq!(inner.retired_screenshot_order.len(), 1);
        drop(inner);
        assert_eq!(
            cache.screenshot_scale(10, Some(20), None),
            Err(ScreenshotContextError::ReplacedOrUnavailable)
        );
    }

    #[test]
    fn replacement_remove_and_clear_run_drop_outside_lock() {
        let cache = Arc::new(ElementCacheCore::new());
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
        let cache = ElementCacheCore::new();
        let low = 7;
        let high = (1_u64 << 32) | low;
        let first = cache.publish(42, low, Payload(vec![10]));
        let second = cache.publish(42, high, Payload(vec![20]));
        let token = token_for(second, 0);
        let resolved = cache
            .resolve_element_args(42, None, Some(&token), None, Some(high), "click")
            .unwrap();
        assert!(
            matches!(resolved, ResolvedElement::Element { window_id: Some(window), element: 20, .. } if window == high)
        );
        assert!(cache
            .resolve_element_args(42, None, Some(&token), None, Some(low), "click")
            .is_err());
        let handle = format!("s{second:08x}");
        assert!(cache
            .resolve_element_args(42, Some(0), None, Some(&handle), Some(high), "click")
            .is_ok());
        assert!(cache
            .resolve_element_args(42, Some(0), None, Some(&handle), Some(low), "click")
            .is_err());
        cache.remove(42, high);
        assert!(cache
            .resolve_element_args(42, None, Some(&token), None, None, "click")
            .is_err());
        assert!(cache
            .resolve_element_args(
                42,
                None,
                Some(&token_for(first, 0)),
                None,
                Some(low),
                "click"
            )
            .is_ok());
    }

    #[test]
    fn bindings_sharing_a_scope_keep_independent_payload_ownership() {
        crate::tool::with_runtime_scope("snapshot-binding-ownership".into(), || {
            let first = Arc::new(ElementCacheCore::new());
            let second = Arc::new(ElementCacheCore::new());
            register_runtime_cache(&first);
            register_runtime_cache(&second);
            let first_id = first.publish(42, 7, Payload(vec![10]));
            let second_id = second.publish(42, 7, Payload(vec![20]));
            assert!(second
                .resolve_element_args(42, None, Some(&token_for(first_id, 0)), None, None, "click")
                .is_err());
            drop(first);
            let resolved = second
                .resolve_element_args(
                    42,
                    None,
                    Some(&token_for(second_id, 0)),
                    None,
                    None,
                    "click",
                )
                .unwrap();
            assert!(matches!(
                resolved,
                ResolvedElement::Element { element: 20, .. }
            ));
            assert!(Arc::ptr_eq(
                &current_runtime_cache::<Payload>().unwrap(),
                &second
            ));
            retire_runtime_scope("snapshot-binding-ownership");
        });
    }

    #[test]
    fn recording_discovery_does_not_extend_payload_lifetime() {
        crate::tool::with_runtime_scope("snapshot-weak-discovery".into(), || {
            let cache = Arc::new(ElementCacheCore::new());
            let drops = Arc::new(AtomicUsize::new(0));
            cache.publish(
                42,
                7,
                DropCounter {
                    owner: Arc::downgrade(&cache),
                    drops: drops.clone(),
                },
            );
            register_runtime_cache(&cache);
            assert_eq!(Arc::strong_count(&cache), 1);
            drop(cache);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert!(current_runtime_cache::<DropCounter>().is_none());
            {
                let caches = runtime_caches().lock().unwrap();
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
        let _cache: ElementCacheCore<Payload> = ElementCacheCore::default();
    }
}
