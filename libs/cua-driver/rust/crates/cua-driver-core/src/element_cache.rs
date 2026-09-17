use crate::element_token::{
    parse_element_args, refusal, ResolvedElement, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
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
    payload: S,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenshotContextError {
    ReplacedOrUnavailable,
}

pub struct ElementCacheCore<S: SnapshotPayload> {
    runtime_scope: String,
    inner: Mutex<HashMap<i32, Vec<Snapshot<S>>>>,
}

impl<S: SnapshotPayload> ElementCacheCore<S> {
    pub fn new() -> Self {
        Self {
            runtime_scope: current_runtime_scope(),
            inner: Mutex::new(HashMap::new()),
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
            let lane = inner.entry(pid).or_default();
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
        let inner = self.inner.lock().unwrap();
        let Some(lane) = inner.get(&pid) else {
            return Ok(None);
        };
        if let Some(window_id) = window_id {
            let Some(snapshot) = lane.iter().find(|entry| entry.window_id == window_id) else {
                return Ok(None);
            };
            if snapshot.screenshot_owner.as_deref() != session {
                return Err(ScreenshotContextError::ReplacedOrUnavailable);
            }
            return snapshot
                .screenshot_scale
                .map(Some)
                .ok_or(ScreenshotContextError::ReplacedOrUnavailable);
        }
        let mut agreed = None;
        for snapshot in lane {
            if snapshot.screenshot_owner.as_deref() != session {
                return Err(ScreenshotContextError::ReplacedOrUnavailable);
            }
            let scale = snapshot
                .screenshot_scale
                .ok_or(ScreenshotContextError::ReplacedOrUnavailable)?;
            match agreed {
                None => agreed = Some(scale),
                Some(previous) if (previous - scale).abs() < 1e-9 => {}
                Some(_) => return Err(ScreenshotContextError::ReplacedOrUnavailable),
            }
        }
        Ok(agreed)
    }

    pub fn screenshot_scale_or_refusal(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<f64, ToolResult> {
        match self.screenshot_scale(pid, window_id, session) {
            Ok(scale) => Ok(scale.unwrap_or(1.0)),
            Err(ScreenshotContextError::ReplacedOrUnavailable) => Err(ToolResult::error(
                "The latest snapshot for this window does not contain a screenshot owned by this session. Call get_window_state with a screenshot on the same connection before using pixels."
            ).with_structured(serde_json::json!({
                "code": "screenshot_context_missing",
                "pid": pid,
                "window_id": window_id
            }))),
        }
    }

    pub fn retire_session_screenshots(&self, session: &str) -> usize {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            let mut retired = Vec::new();
            for lane in inner.values_mut() {
                let mut index = 0;
                while index < lane.len() {
                    if lane[index].screenshot_owner.as_deref() == Some(session) {
                        retired.push(lane.remove(index));
                    } else {
                        index += 1;
                    }
                }
            }
            inner.retain(|_, lane| !lane.is_empty());
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
            inner.get_mut(&pid).and_then(|lane| {
                let position = lane.iter().position(|entry| entry.window_id == window_id)?;
                Some(lane.remove(position))
            })
        };
        drop(retired);
    }

    pub fn clear(&self) -> usize {
        let retired = std::mem::take(&mut *self.inner.lock().unwrap());
        retired.len()
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
            Ok(None)
        );
        assert_eq!(
            cache.screenshot_scale(10, Some(21), Some("survivor")),
            Ok(Some(2.0))
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
