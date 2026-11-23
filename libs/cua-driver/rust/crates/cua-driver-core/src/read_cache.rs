// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in stale-while-revalidate cache for `list_windows` replies.
//!
//! An agent loop that calls `list_windows` back to back recomputes the same
//! enumeration each time. A host that accepts a bounded staleness can opt in
//! at trusted launch through the daemon environment:
//!
//! - [`LIST_WINDOWS_CACHE_ENV`] (`CUA_DRIVER_LIST_WINDOWS_CACHE_MS`): how long
//!   a reply stays fresh, in milliseconds. A reply older than that but younger
//!   than [`STALE_CAP_FACTOR`] times it is still served, and the first such
//!   call starts one background recompute for its arguments. Older replies are
//!   recomputed live.
//!
//! Unset, empty, `0`, or unparsable values leave the tool uncached, so a daemon
//! launched without the variable behaves exactly as before. No tool argument
//! can change the bound. Any dispatch of a tool that is not read-only calls
//! [`note_actuation`], and no reply computed before that is served again, so a
//! read after an action always reflects the action. Only the public tool is
//! wrapped; post-action observation and `verify_state` keep reading live.

use crate::protocol::{Content, ToolResult};
use crate::tool::{Tool, ToolDef};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a `list_windows` reply stays fresh, in milliseconds.
pub const LIST_WINDOWS_CACHE_ENV: &str = "CUA_DRIVER_LIST_WINDOWS_CACHE_MS";

/// Largest accepted freshness. Larger values are clamped so a typo cannot
/// make replies minutes old.
pub const MAX_READ_CACHE_TTL: Duration = Duration::from_millis(1_000);

/// A reply is served stale until it is this many freshness periods old.
pub const STALE_CAP_FACTOR: u32 = 20;

/// Distinct argument sets kept at once. The oldest is evicted past this.
const MAX_ENTRIES: usize = 64;

static ACTUATION_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Retire every cached reply. Dispatch calls this before and after any tool
/// that is not read-only, so no reply computed before an action, or while it
/// ran, is served after it.
pub fn note_actuation() {
    ACTUATION_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Parse the freshness bound. `None` means caching is off.
pub fn ttl_from_raw(raw: Option<&str>) -> Option<Duration> {
    let millis = raw?.trim().parse::<u64>().ok()?;
    (millis > 0).then(|| Duration::from_millis(millis).min(MAX_READ_CACHE_TTL))
}

/// Register `tool` behind the reply cache when the daemon environment opts
/// in, or as is otherwise.
pub fn list_windows_reply_cache(tool: Arc<dyn Tool>) -> Box<dyn Tool> {
    let raw = std::env::var(LIST_WINDOWS_CACHE_ENV).ok();
    match ttl_from_raw(raw.as_deref()) {
        Some(ttl) => Box::new(CachedReadTool::new(tool, ttl)),
        None => Box::new(SharedTool(tool)),
    }
}

struct SharedTool(Arc<dyn Tool>);

#[async_trait]
impl Tool for SharedTool {
    fn def(&self) -> &ToolDef {
        self.0.def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        self.0.invoke(args).await
    }
}

/// A successful reply, kept in a form that can be handed out repeatedly.
#[derive(Clone)]
struct Reply {
    content: Vec<Content>,
    structured_content: Option<Value>,
}

impl Reply {
    fn of(result: &ToolResult) -> Option<Self> {
        if result.is_error == Some(true) || result.action_record.is_some() {
            return None;
        }
        Some(Self {
            content: result.content.clone(),
            structured_content: result.structured_content.clone(),
        })
    }

    fn into_result(self) -> ToolResult {
        ToolResult {
            content: self.content,
            structured_content: self.structured_content,
            ..ToolResult::default()
        }
    }
}

struct Entry<V> {
    value: V,
    /// When the computation that produced `value` started.
    started: Instant,
    generation: u64,
    refreshing: bool,
}

#[derive(Debug, PartialEq)]
enum Lookup<V> {
    Fresh(V),
    /// Serve `value`; when `refresh` is true this caller owns the one
    /// background recompute for the key.
    Stale {
        value: V,
        refresh: bool,
    },
    Miss,
}

/// Replies keyed by argument set. Callers pass the clock and generation, so
/// the policy is exercised without sleeping.
struct ReplyCache<V> {
    ttl: Duration,
    entries: Mutex<HashMap<String, Entry<V>>>,
}

impl<V: Clone> ReplyCache<V> {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry<V>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lookup(&self, key: &str, now: Instant, generation: u64) -> Lookup<V> {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(key) else {
            return Lookup::Miss;
        };
        if entry.generation != generation {
            entries.remove(key);
            return Lookup::Miss;
        }
        let age = now.saturating_duration_since(entry.started);
        if age < self.ttl {
            return Lookup::Fresh(entry.value.clone());
        }
        if age < self.ttl * STALE_CAP_FACTOR {
            let refresh = !entry.refreshing;
            entry.refreshing = true;
            return Lookup::Stale {
                value: entry.value.clone(),
                refresh,
            };
        }
        entries.remove(key);
        Lookup::Miss
    }

    /// Record a computation that started at `started` under `generation`.
    /// `None` (an error) only ends the key's refresh, and a computation never
    /// replaces a newer one. [`Self::lookup`] drops a retired generation.
    fn store(&self, key: String, value: Option<V>, started: Instant, generation: u64) {
        let mut entries = self.lock();
        let current = entries.get(&key);
        let newer_kept = current.is_some_and(|entry| entry.started > started);
        let Some(value) = value.filter(|_| !newer_kept) else {
            if let Some(entry) = entries.get_mut(&key) {
                entry.refreshing = false;
            }
            return;
        };
        if current.is_none() && entries.len() >= MAX_ENTRIES {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.started)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            Entry {
                value,
                started,
                generation,
                refreshing: false,
            },
        );
    }
}

/// A read-only tool whose successful replies are served from a
/// [`ReplyCache`] keyed by the complete argument object, including the
/// session fields dispatch adds, so sessions never share an entry.
pub struct CachedReadTool {
    inner: Arc<dyn Tool>,
    cache: Arc<ReplyCache<Reply>>,
    generation: &'static AtomicU64,
}

impl CachedReadTool {
    pub fn new(inner: Arc<dyn Tool>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: Arc::new(ReplyCache::new(ttl)),
            generation: &ACTUATION_GENERATION,
        }
    }

    async fn compute(
        inner: &Arc<dyn Tool>,
        cache: &ReplyCache<Reply>,
        generation: u64,
        key: String,
        args: Value,
    ) -> ToolResult {
        let started = Instant::now();
        let result = inner.invoke(args).await;
        cache.store(key, Reply::of(&result), started, generation);
        result
    }
}

#[async_trait]
impl Tool for CachedReadTool {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let key = args.to_string();
        let generation = self.generation.load(Ordering::SeqCst);
        match self.cache.lookup(&key, Instant::now(), generation) {
            Lookup::Fresh(reply) => reply.into_result(),
            Lookup::Stale { value, refresh } => {
                if refresh {
                    let inner = Arc::clone(&self.inner);
                    let cache = Arc::clone(&self.cache);
                    tokio::spawn(async move {
                        Self::compute(&inner, &cache, generation, key, args).await;
                    });
                }
                value.into_result()
            }
            Lookup::Miss => Self::compute(&self.inner, &self.cache, generation, key, args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_millis(100);

    fn fresh_cache() -> (ReplyCache<u32>, Instant, u64) {
        (ReplyCache::new(TTL), Instant::now(), 0)
    }

    #[test]
    fn unset_zero_or_malformed_values_leave_caching_off() {
        for raw in [
            None,
            Some(""),
            Some("0"),
            Some("-1"),
            Some("1.5"),
            Some("100ms"),
        ] {
            assert_eq!(ttl_from_raw(raw), None, "{raw:?}");
        }
        assert_eq!(ttl_from_raw(Some(" 100 ")), Some(TTL));
        assert_eq!(ttl_from_raw(Some("60000")), Some(MAX_READ_CACHE_TTL));
    }

    #[test]
    fn a_reply_is_fresh_then_stale_with_one_refresh_then_recomputed() {
        let (cache, t0, generation) = fresh_cache();
        assert_eq!(cache.lookup("k", t0, generation), Lookup::Miss);
        cache.store("k".into(), Some(1), t0, generation);

        let fresh = t0 + TTL - Duration::from_millis(1);
        assert_eq!(cache.lookup("k", fresh, generation), Lookup::Fresh(1));

        let stale = t0 + TTL;
        assert_eq!(
            cache.lookup("k", stale, generation),
            Lookup::Stale {
                value: 1,
                refresh: true
            }
        );
        assert_eq!(
            cache.lookup("k", stale, generation),
            Lookup::Stale {
                value: 1,
                refresh: false
            }
        );

        let capped = t0 + TTL * STALE_CAP_FACTOR;
        assert_eq!(cache.lookup("k", capped, generation), Lookup::Miss);
    }

    #[test]
    fn a_finished_refresh_serves_fresh_and_a_failed_one_can_retry() {
        let (cache, t0, generation) = fresh_cache();
        cache.store("k".into(), Some(1), t0, generation);
        let stale = t0 + TTL * 2;
        assert!(matches!(
            cache.lookup("k", stale, generation),
            Lookup::Stale { refresh: true, .. }
        ));

        cache.store("k".into(), None, stale, generation);
        assert_eq!(
            cache.lookup("k", stale, generation),
            Lookup::Stale {
                value: 1,
                refresh: true
            }
        );

        cache.store("k".into(), Some(2), stale, generation);
        assert_eq!(cache.lookup("k", stale, generation), Lookup::Fresh(2));
    }

    #[test]
    fn an_older_computation_never_replaces_a_newer_reply() {
        let (cache, t0, generation) = fresh_cache();
        cache.store("k".into(), Some(2), t0 + TTL, generation);
        cache.store("k".into(), Some(1), t0, generation);
        assert_eq!(cache.lookup("k", t0 + TTL, generation), Lookup::Fresh(2));
    }

    #[test]
    fn a_late_refresh_never_releases_a_newer_entrys_refresh() {
        let (cache, t0, generation) = fresh_cache();
        cache.store("k".into(), Some(1), t0, generation);
        let first_refresh = t0 + TTL * 2;
        assert!(matches!(
            cache.lookup("k", first_refresh, generation),
            Lookup::Stale { refresh: true, .. }
        ));
        let replaced = t0 + TTL * STALE_CAP_FACTOR;
        assert_eq!(cache.lookup("k", replaced, generation), Lookup::Miss);
        cache.store("k".into(), Some(2), replaced, generation);
        assert!(matches!(
            cache.lookup("k", replaced + TTL, generation),
            Lookup::Stale { refresh: true, .. }
        ));

        cache.store("k".into(), None, first_refresh, generation);
        assert!(matches!(
            cache.lookup("k", replaced + TTL * 2, generation),
            Lookup::Stale { refresh: false, .. }
        ));
    }

    #[test]
    fn a_refresh_from_before_an_action_never_replaces_a_reply_from_after_it() {
        let (cache, t0, generation) = fresh_cache();
        cache.store("k".into(), Some(2), t0, generation + 1);
        cache.store("k".into(), Some(1), t0 + TTL, generation);
        assert_eq!(
            cache.lookup("k", t0 + TTL, generation + 1),
            Lookup::Stale {
                value: 2,
                refresh: true
            }
        );
    }

    #[test]
    fn keys_are_independent_and_bounded() {
        let (cache, t0, generation) = fresh_cache();
        for i in 0..=MAX_ENTRIES as u32 {
            cache.store(
                i.to_string(),
                Some(i),
                t0 + Duration::from_micros(i.into()),
                generation,
            );
        }
        assert_eq!(cache.lock().len(), MAX_ENTRIES);
        assert_eq!(cache.lookup("0", t0, generation), Lookup::Miss);
        assert_eq!(cache.lookup("1", t0, generation), Lookup::Fresh(1));
    }

    struct Counting {
        def: ToolDef,
        calls: AtomicU64,
    }

    #[async_trait]
    impl Tool for Counting {
        fn def(&self) -> &ToolDef {
            &self.def
        }

        async fn invoke(&self, args: Value) -> ToolResult {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if args.get("fail").is_some() {
                return ToolResult::error("boom");
            }
            ToolResult::text(format!("call {call}"))
        }
    }

    fn counting() -> Arc<Counting> {
        Arc::new(Counting {
            def: ToolDef {
                name: "list_windows".into(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            calls: AtomicU64::new(0),
        })
    }

    fn text(result: &ToolResult) -> String {
        serde_json::to_value(&result.content).unwrap()[0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn an_actuation_retires_cached_replies_and_errors_are_never_cached() {
        static GENERATION: AtomicU64 = AtomicU64::new(0);
        let inner = counting();
        let tool = CachedReadTool {
            generation: &GENERATION,
            ..CachedReadTool::new(inner.clone(), Duration::from_secs(1))
        };
        let args = serde_json::json!({"pid": 1});

        assert_eq!(text(&tool.invoke(args.clone()).await), "call 0");
        assert_eq!(text(&tool.invoke(args.clone()).await), "call 0");
        assert_eq!(
            text(&tool.invoke(serde_json::json!({"pid": 2})).await),
            "call 1"
        );

        GENERATION.fetch_add(1, Ordering::SeqCst);
        assert_eq!(text(&tool.invoke(args.clone()).await), "call 2");

        let failing = serde_json::json!({"fail": true});
        assert_eq!(tool.invoke(failing.clone()).await.is_error, Some(true));
        assert_eq!(tool.invoke(failing).await.is_error, Some(true));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 5);
    }
}
