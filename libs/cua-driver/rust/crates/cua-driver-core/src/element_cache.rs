use crate::element_token::{
    parse_element_args, refusal, ResolvedElement, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
};
use crate::protocol::ToolResult;
use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub trait SnapshotPayload: Send + Sync + 'static {
    type Element;
    fn len(&self) -> usize;
    fn retain(&self, index: usize) -> Option<Self::Element>;
    fn resolve_fresh(
        _pid: i32,
        _window_id: u64,
        _index: usize,
    ) -> Result<Option<Self::Element>, String> {
        Ok(None)
    }
}

struct SnapshotAddress {
    id: u32,
    window_id: u64,
}

/// Runtime-scoped token addresses. Observed accessibility payloads are never stored.
pub struct ElementCacheCore<S: SnapshotPayload> {
    runtime_scope: String,
    inner: Mutex<HashMap<i32, Vec<SnapshotAddress>>>,
    payload: PhantomData<fn() -> S>,
}

impl<S: SnapshotPayload> ElementCacheCore<S> {
    pub fn new() -> Self {
        Self {
            runtime_scope: current_runtime_scope(),
            inner: Mutex::new(HashMap::new()),
            payload: PhantomData,
        }
    }
    pub fn publish(&self, pid: i32, window_id: u64, payload: S) -> u32 {
        drop(payload);
        let mut inner = self.inner.lock().unwrap();
        let lane = inner.entry(pid).or_default();
        if let Some(position) = lane.iter().position(|entry| entry.window_id == window_id) {
            lane.remove(position);
        }
        if lane.len() == LRU_CAP_PER_PID {
            lane.remove(0);
        }
        let id = crate::element_token::mint_snapshot_id();
        lane.push(SnapshotAddress { id, window_id });
        id
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
        let resolved_window = {
            let inner = self.inner.lock().unwrap();
            inner
                .get(&pid)
                .and_then(|lane| lane.iter().find(|entry| entry.id == reference.snapshot_id))
                .map(|entry| entry.window_id)
        };
        let Some(resolved_window) = resolved_window else {
            let caches = runtime_caches()
                .lock()
                .unwrap()
                .iter()
                .filter(|(scope, _)| *scope != &self.runtime_scope)
                .filter_map(|(_, cache)| cache.upgrade())
                .collect::<Vec<_>>();
            return Err(
                if caches
                    .iter()
                    .any(|cache| cache.contains(pid, reference.snapshot_id))
                {
                    refusal(
                        "generation_mismatch",
                        "element_token belongs to another runtime generation".into(),
                    )
                } else {
                    refusal("stale_element_token", STALE_TOKEN_ERROR.into())
                },
            );
        };
        reference.validate_window(resolved_window, tool_name)?;
        let element=S::resolve_fresh(pid,resolved_window,reference.element_index).map_err(|message|refusal("element_resolution_failed",message))?.ok_or_else(||refusal("invalid_element_token",format!("element_token element_index {} does not identify an actionable element in the current accessibility state",reference.element_index)))?;
        Ok(ResolvedElement::Element {
            window_id: Some(resolved_window),
            element_index: reference.element_index,
            via_token: reference.via_token,
            element,
        })
    }
    pub fn remove(&self, pid: i32, window_id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(lane) = inner.get_mut(&pid) {
            if let Some(position) = lane.iter().position(|entry| entry.window_id == window_id) {
                lane.remove(position);
            }
        }
    }
    pub fn clear(&self) -> usize {
        std::mem::take(&mut *self.inner.lock().unwrap()).len()
    }
}
impl<S: SnapshotPayload> Default for ElementCacheCore<S> {
    fn default() -> Self {
        Self::new()
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn state() -> &'static Mutex<HashMap<(i32, u64), Vec<usize>>> {
        static STATE: OnceLock<Mutex<HashMap<(i32, u64), Vec<usize>>>> = OnceLock::new();
        STATE.get_or_init(|| Mutex::new(HashMap::new()))
    }
    struct Payload {
        observed: Vec<usize>,
        drops: Option<Arc<AtomicUsize>>,
    }
    impl SnapshotPayload for Payload {
        type Element = usize;
        fn len(&self) -> usize {
            self.observed.len()
        }
        fn retain(&self, index: usize) -> Option<usize> {
            self.observed.get(index).copied()
        }
        fn resolve_fresh(pid: i32, window_id: u64, index: usize) -> Result<Option<usize>, String> {
            Ok(state()
                .lock()
                .unwrap()
                .get(&(pid, window_id))
                .and_then(|elements| elements.get(index).copied()))
        }
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            if let Some(drops) = &self.drops {
                drops.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    fn payload(observed: Vec<usize>) -> Payload {
        Payload {
            observed,
            drops: None,
        }
    }
    #[test]
    fn publishing_observation_does_not_retain_accessibility_payload() {
        let cache = ElementCacheCore::new();
        let drops = Arc::new(AtomicUsize::new(0));
        cache.publish(
            42,
            7,
            Payload {
                observed: vec![10],
                drops: Some(drops.clone()),
            },
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn action_resolution_uses_fresh_accessibility_state() {
        let cache = ElementCacheCore::new();
        let snapshot = cache.publish(43, 8, payload(vec![10]));
        state().lock().unwrap().insert((43, 8), vec![99]);
        let resolved = cache
            .resolve_element_args(43, None, Some(&token_for(snapshot, 0)), None, None, "click")
            .unwrap();
        assert!(matches!(
            resolved,
            ResolvedElement::Element { element: 99, .. }
        ));
    }
    #[test]
    fn missing_fresh_address_refuses_before_action() {
        let cache = ElementCacheCore::new();
        let snapshot = cache.publish(44, 9, payload(vec![10]));
        state().lock().unwrap().insert((44, 9), vec![]);
        let error = cache
            .resolve_element_args(44, None, Some(&token_for(snapshot, 0)), None, None, "click")
            .unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["refusal"]["code"],
            "invalid_element_token"
        );
    }
}
