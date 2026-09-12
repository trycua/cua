use cua_driver_core::element_cache::{
    register_runtime_cache, retire_runtime_scope, ElementCacheCore, SnapshotPayload,
};
use cua_driver_core::element_token::{
    format_token, ResolvedElement, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
};
use cua_driver_core::tool::with_runtime_scope;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

struct Payload<T: Clone + Send + Sync + 'static> {
    elements: Vec<T>,
    drops: Option<Arc<AtomicUsize>>,
}

impl<T: Clone + Send + Sync + 'static> SnapshotPayload for Payload<T> {
    type Element = T;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<T> {
        self.elements.get(index).cloned()
    }
}

impl<T: Clone + Send + Sync + 'static> Drop for Payload<T> {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn payload<T: Clone + Send + Sync + 'static>(elements: Vec<T>) -> Payload<T> {
    Payload {
        elements,
        drops: None,
    }
}

fn resolve<T: Clone + Send + Sync + 'static>(
    cache: &ElementCacheCore<Payload<T>>,
    snapshot: u32,
    index: usize,
) -> Result<(u64, usize, T), String> {
    cache
        .resolve_element_args(
            42,
            None,
            Some(&format_token(snapshot, index)),
            None,
            None,
            "click",
        )
        .map(|result| match result {
            ResolvedElement::Element {
                window_id: Some(window),
                element_index,
                element,
                ..
            } => (window, element_index, element),
            _ => panic!("expected element"),
        })
        .map_err(|error| {
            error.structured_content.unwrap()["refusal"]["message"]
                .as_str()
                .unwrap()
                .to_owned()
        })
}

#[test]
fn empty_snapshot_has_no_resolvable_members() {
    let cache = ElementCacheCore::new();
    let snapshot = cache.publish(42, 7, payload(Vec::<usize>::new()));
    assert!(
        resolve(&cache, snapshot, 0).is_err(),
        "an empty snapshot must not admit element zero"
    );
}

#[test]
fn replacement_invalidates_every_old_member_and_admits_new_members() {
    let cache = ElementCacheCore::new();
    let first = cache.publish(42, 7, payload(vec![0, 1]));
    let second = cache.publish(42, 7, payload(vec![10, 11]));
    assert_ne!(first, second);
    for index in 0..2 {
        assert_eq!(
            resolve(&cache, first, index),
            Err(STALE_TOKEN_ERROR.to_owned())
        );
        assert_eq!(resolve(&cache, second, index), Ok((7, index, index + 10)));
    }
}

#[test]
fn resolving_does_not_change_publication_order_eviction() {
    let cache = ElementCacheCore::new();
    let first = cache.publish(42, 1, payload(vec![1]));
    for window in 2..=LRU_CAP_PER_PID as u64 {
        cache.publish(42, window, payload(vec![1]));
    }
    assert_eq!(resolve(&cache, first, 0), Ok((1, 0, 1)));
    let latest = cache.publish(42, LRU_CAP_PER_PID as u64 + 1, payload(vec![1]));
    assert_eq!(resolve(&cache, first, 0), Err(STALE_TOKEN_ERROR.to_owned()));
    assert!(resolve(&cache, latest, 0).is_ok());
}

#[test]
fn clearing_one_runtime_preserves_other_runtime_same_window() {
    let make = |scope: &str| {
        with_runtime_scope(scope.into(), || {
            let cache = Arc::new(ElementCacheCore::new());
            register_runtime_cache(&cache);
            let id = cache.publish(42, 7, payload(vec![1]));
            (cache, id)
        })
    };
    let (first_cache, first) = make("invariant-a");
    let (second_cache, second) = make("invariant-b");
    with_runtime_scope("invariant-b".into(), || {
        assert!(resolve(&second_cache, first, 0)
            .unwrap_err()
            .contains("another runtime generation"));
        assert!(resolve(&first_cache, first, 0)
            .unwrap_err()
            .contains("another runtime generation"));
    });
    assert_eq!(retire_runtime_scope("invariant-a"), 1);
    assert_eq!(retire_runtime_scope("invariant-a"), 0);
    with_runtime_scope("invariant-a".into(), || {
        assert!(resolve(&first_cache, first, 0).is_err());
    });
    with_runtime_scope("invariant-b".into(), || {
        assert_eq!(resolve(&second_cache, second, 0), Ok((7, 0, 1)));
    });
    retire_runtime_scope("invariant-b");
}

#[test]
fn token_resolution_cannot_be_retargeted_by_cache_replacement() {
    let cache = ElementCacheCore::new();
    let snapshot = cache.publish(42, 7, payload(vec!["original-target"]));
    let (resolved_tx, resolved_rx) = mpsc::channel();
    let (replaced_tx, replaced_rx) = mpsc::channel();
    let observed = std::thread::scope(|threads| {
        let cache = &cache;
        let action = threads.spawn(move || {
            let target = resolve(cache, snapshot, 0);
            resolved_tx.send(()).unwrap();
            replaced_rx.recv().unwrap();
            target.ok().map(|(_, _, element)| element)
        });
        resolved_rx.recv().unwrap();
        cache.publish(42, 7, payload(vec!["replacement-target"]));
        replaced_tx.send(()).unwrap();
        action.join().unwrap()
    });
    assert!(
        observed.is_none() || observed == Some("original-target"),
        "resolved identity was combined with another payload: {observed:?}"
    );
}

#[test]
fn runtime_retirement_releases_unadmitted_cache_payload() {
    let drops = Arc::new(AtomicUsize::new(0));
    let cache = with_runtime_scope("retirement-invariant".into(), || {
        let cache = Arc::new(ElementCacheCore::new());
        register_runtime_cache(&cache);
        cache.publish(
            42,
            7,
            Payload {
                elements: vec![1],
                drops: Some(drops.clone()),
            },
        );
        cache
    });
    retire_runtime_scope("retirement-invariant");
    let released_at_retirement = drops.load(Ordering::SeqCst);
    drop(cache);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        released_at_retirement, 1,
        "retired identity must release its unadmitted payload"
    );
}

#[test]
fn eviction_releases_unadmitted_cache_payload() {
    let cache = ElementCacheCore::new();
    let drops = Arc::new(AtomicUsize::new(0));
    for window in 0..=LRU_CAP_PER_PID as u64 {
        cache.publish(
            42,
            window,
            Payload {
                elements: vec![1],
                drops: Some(drops.clone()),
            },
        );
    }
    let released_at_eviction = drops.load(Ordering::SeqCst);
    drop(cache);
    assert_eq!(drops.load(Ordering::SeqCst), LRU_CAP_PER_PID + 1);
    assert_eq!(
        released_at_eviction, 1,
        "eviction must release the corresponding unadmitted payload"
    );
}
