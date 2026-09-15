use cua_driver_core::element_cache::{
    register_runtime_cache, retire_runtime_scope, ElementCacheCore, SnapshotPayload,
};
use cua_driver_core::element_token::{format_token, ResolvedElement, LRU_CAP_PER_PID};
use cua_driver_core::tool::with_runtime_scope;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
            .and_then(|v| v.get(index).copied()))
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        if let Some(d) = &self.drops {
            d.fetch_add(1, Ordering::SeqCst);
        }
    }
}
fn payload(v: Vec<usize>) -> Payload {
    Payload {
        observed: v,
        drops: None,
    }
}
fn resolve(
    cache: &ElementCacheCore<Payload>,
    pid: i32,
    snapshot: u32,
    index: usize,
) -> Result<(u64, usize, usize), String> {
    cache
        .resolve_element_args(
            pid,
            None,
            Some(&format_token(snapshot, index)),
            None,
            None,
            "click",
        )
        .map(|r| match r {
            ResolvedElement::Element {
                window_id: Some(w),
                element_index,
                element,
                ..
            } => (w, element_index, element),
            _ => panic!(),
        })
        .map_err(|e| {
            e.structured_content.unwrap()["refusal"]["code"]
                .as_str()
                .unwrap()
                .into()
        })
}
#[test]
fn actions_use_current_state() {
    let c = ElementCacheCore::new();
    let id = c.publish(101, 7, payload(vec![10]));
    state().lock().unwrap().insert((101, 7), vec![20]);
    assert_eq!(resolve(&c, 101, id, 0), Ok((7, 0, 20)));
}
#[test]
fn removed_current_element_refuses() {
    let c = ElementCacheCore::new();
    let id = c.publish(102, 8, payload(vec![10]));
    state().lock().unwrap().insert((102, 8), vec![]);
    assert_eq!(resolve(&c, 102, id, 0), Err("invalid_element_token".into()));
}
#[test]
fn replacement_and_eviction_retire_addresses() {
    let c = ElementCacheCore::new();
    let old = c.publish(103, 1, payload(vec![1]));
    let new = c.publish(103, 1, payload(vec![2]));
    state().lock().unwrap().insert((103, 1), vec![3]);
    assert_eq!(resolve(&c, 103, old, 0), Err("stale_element_token".into()));
    assert_eq!(resolve(&c, 103, new, 0), Ok((1, 0, 3)));
    for w in 2..=LRU_CAP_PER_PID as u64 + 1 {
        c.publish(103, w, payload(vec![1]));
    }
    assert_eq!(resolve(&c, 103, new, 0), Err("stale_element_token".into()));
}
#[test]
fn retirement_never_owns_payloads() {
    let drops = Arc::new(AtomicUsize::new(0));
    let c = with_runtime_scope("cacheless-retirement".into(), || {
        let c = Arc::new(ElementCacheCore::new());
        register_runtime_cache(&c);
        c.publish(
            104,
            9,
            Payload {
                observed: vec![1],
                drops: Some(drops.clone()),
            },
        );
        c
    });
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(retire_runtime_scope("cacheless-retirement"), 1);
    drop(c);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
