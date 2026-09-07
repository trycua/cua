use cua_driver_core::element_cache::ElementCacheCore;
use cua_driver_core::element_token::{
    format_token, TokenRegistry, LRU_CAP_PER_PID, STALE_TOKEN_ERROR,
};
use cua_driver_core::tool::with_runtime_scope;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

#[test]
fn empty_snapshot_has_no_resolvable_members() {
    let registry = TokenRegistry::default();
    let snapshot = registry.register_snapshot(42, 7, 0);
    assert!(
        registry.resolve(42, &format_token(snapshot, 0)).is_err(),
        "an empty snapshot must not admit element zero"
    );
}

#[test]
fn replacement_invalidates_every_old_member_and_admits_new_members() {
    let registry = TokenRegistry::default();
    let first = registry.register_snapshot(42, 7, 2);
    let second = registry.register_snapshot(42, 7, 2);
    assert_ne!(first, second);
    for index in 0..2 {
        assert_eq!(
            registry.resolve(42, &format_token(first, index)),
            Err(STALE_TOKEN_ERROR.to_owned())
        );
        assert_eq!(
            registry.resolve(42, &format_token(second, index)),
            Ok((7, index))
        );
    }
}

#[test]
fn resolving_does_not_change_publication_order_eviction() {
    let registry = TokenRegistry::default();
    let first = registry.register_snapshot(42, 1, 1);
    for window in 2..=LRU_CAP_PER_PID as u32 {
        registry.register_snapshot(42, window, 1);
    }
    assert_eq!(registry.resolve(42, &format_token(first, 0)), Ok((1, 0)));
    let latest = registry.register_snapshot(42, LRU_CAP_PER_PID as u32 + 1, 1);
    assert_eq!(
        registry.resolve(42, &format_token(first, 0)),
        Err(STALE_TOKEN_ERROR.to_owned())
    );
    assert!(registry.resolve(42, &format_token(latest, 0)).is_ok());
}

#[test]
fn clearing_one_runtime_preserves_other_runtime_same_window() {
    let registry = TokenRegistry::default();
    let first = with_runtime_scope("invariant-a".into(), || {
        registry.register_snapshot(42, 7, 1)
    });
    let second = with_runtime_scope("invariant-b".into(), || {
        registry.register_snapshot(42, 7, 1)
    });
    assert_eq!(registry.clear_runtime_scope("invariant-a"), 1);
    assert_eq!(registry.clear_runtime_scope("invariant-a"), 0);
    with_runtime_scope("invariant-a".into(), || {
        assert!(registry.resolve(42, &format_token(first, 0)).is_err());
    });
    with_runtime_scope("invariant-b".into(), || {
        assert_eq!(registry.resolve(42, &format_token(second, 0)), Ok((7, 0)));
    });
}

#[test]
fn token_resolution_cannot_be_retargeted_by_cache_replacement() {
    let registry = TokenRegistry::default();
    let cache = ElementCacheCore::new();
    cache.insert((42, 7), vec!["original-target"]);
    let snapshot = registry.register_snapshot(42, 7, 1);
    let (resolved_tx, resolved_rx) = mpsc::channel();
    let (replaced_tx, replaced_rx) = mpsc::channel();
    let observed = std::thread::scope(|threads| {
        let registry = &registry;
        let cache = &cache;
        let action = threads.spawn(move || {
            let target = registry.resolve(42, &format_token(snapshot, 0));
            resolved_tx.send(()).unwrap();
            replaced_rx.recv().unwrap();
            target.ok().and_then(|(window, index)| {
                cache.with_snapshot(&(42, window), |payload| payload[index])
            })
        });
        resolved_rx.recv().unwrap();
        cache.insert((42, 7), vec!["replacement-target"]);
        registry.register_snapshot(42, 7, 1);
        replaced_tx.send(()).unwrap();
        action.join().unwrap()
    });
    assert!(
        observed.is_none() || observed == Some("original-target"),
        "resolved identity was combined with another payload: {observed:?}"
    );
}

struct Payload(Arc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn runtime_retirement_releases_unadmitted_cache_payload() {
    let registry = TokenRegistry::default();
    let cache = ElementCacheCore::new();
    let drops = Arc::new(AtomicUsize::new(0));
    with_runtime_scope("retirement-invariant".into(), || {
        cache.insert((42, 7), Payload(drops.clone()));
        registry.register_snapshot(42, 7, 1);
    });
    registry.clear_runtime_scope("retirement-invariant");
    let released_at_retirement = drops.load(Ordering::SeqCst);
    drop(cache);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        released_at_retirement, 1,
        "retired token metadata left its unadmitted payload owned by another store"
    );
}

#[test]
fn eviction_releases_unadmitted_cache_payload() {
    let registry = TokenRegistry::default();
    let cache = ElementCacheCore::new();
    let drops = Arc::new(AtomicUsize::new(0));
    for window in 0..=LRU_CAP_PER_PID as u32 {
        cache.insert((42, window), Payload(drops.clone()));
        registry.register_snapshot(42, window, 1);
    }
    let released_at_eviction = drops.load(Ordering::SeqCst);
    drop(cache);
    assert_eq!(drops.load(Ordering::SeqCst), LRU_CAP_PER_PID + 1);
    assert_eq!(
        released_at_eviction, 1,
        "token eviction must release the corresponding unadmitted cache payload"
    );
}
