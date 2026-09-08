use cua_driver_core::element_cache::{ElementCacheCore, SnapshotPayload};
use cua_driver_core::element_token::{token_for, ResolvedElement};
use cua_driver_core::tool::with_runtime_scope;
use std::hint::black_box;
use std::time::Instant;

struct Payload(Vec<usize>);

impl SnapshotPayload for Payload {
    type Element = usize;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn retain(&self, index: usize) -> Option<usize> {
        self.0.get(index).copied()
    }
}

struct Fixture {
    cache: ElementCacheCore<Payload>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            cache: ElementCacheCore::new(),
        }
    }

    fn publish(&self, window: u64) -> u32 {
        self.cache.publish(731_349, window, Payload(vec![17; 64]))
    }

    fn resolve(&self, token: &str) -> usize {
        let target = self
            .cache
            .resolve_element_args(731_349, None, Some(token), None, None, "click")
            .unwrap();
        match target {
            ResolvedElement::Element { element, .. } => element,
            _ => panic!("expected an element"),
        }
    }
}

fn samples(name: &str, mut operation: impl FnMut()) {
    let iterations = 1_000_000;
    let mut times = Vec::new();
    for sample in 0..45 {
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        if sample >= 5 {
            times.push(start.elapsed().as_nanos() as f64 / iterations as f64);
        }
    }
    println!(
        "{}",
        serde_json::json!({"workload": name, "iterations_per_sample": iterations, "ns_per_operation": times})
    );
}

#[test]
#[ignore = "explicit release-mode latency measurement"]
fn snapshot_cache_latency() {
    with_runtime_scope("snapshot-latency".into(), || {
        let fixture = Fixture::new();
        let token = token_for(fixture.publish(7), 31);
        samples("exact_target_acquisition", || {
            black_box(fixture.resolve(black_box(&token)));
        });
        samples("same_window_publication_64_members", || {
            black_box(fixture.publish(7));
        });
        let mut window = 0;
        samples("publication_acquisition_eviction_64_members", || {
            window = (window + 1) % 16;
            let token = token_for(fixture.publish(window), 31);
            black_box(fixture.resolve(&token));
        });
        fixture.cache.clear();
    });
}
