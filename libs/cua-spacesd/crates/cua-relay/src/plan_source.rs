// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Plans from the Spaces API's entitlements (CUA-1659).
//!
//! The meter asks a [`PlanSource`] for each user's plan and team. This one
//! reads `GET /internal/v1/entitlements/{sub}` (Stripe Entitlements behind
//! the Spaces API) with the relay's service token, the same client as the
//! organization access checks:
//!
//! - the person's plan is the answer's `plan` (`free`, `pro`, `teams`);
//! - a paying Teams organization in `orgs[]` pools its members: keyed by the
//!   organization id, one per-seat allowance per paid seat.
//!
//! Allowances are soft (a notice, never a refusal), so this source fails
//! open: answers are cached for a short TTL and refreshed in the background,
//! an error keeps the last known plan, and only a user never answered for
//! is metered on the default (free) plan. [`PlanSource::assignment`] stays
//! synchronous and never waits on the network; when an answer arrives that
//! changes someone's plan, the meter is told to re-resolve them.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::meter::{Assignment, Membership, PlanSource, TeamPlan, DEFAULT_PLAN};
use crate::org_access::OrgAccess;

/// Default lifetime of a cached plan.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);
/// Wait before asking again after a failed lookup.
const RETRY_AFTER: Duration = Duration::from_secs(15);
/// Most users cached (stale entries are dropped first).
const MAX_ENTRIES: usize = 50_000;
/// Lookups in flight at once. A restart makes every connected user ask at
/// the same moment; the API's Stripe search is rate limited (about 20/s),
/// so an unbounded burst answered 503 (CUA-1781).
const MAX_CONCURRENT_LOOKUPS: usize = 4;
/// Wait before the first retry for a user never answered for.
const RETRY_UNKNOWN_AFTER: Duration = Duration::from_secs(3);
/// Tries per lookup. A transport blip (a connection reset or a 5 s stall on
/// one pod) usually clears within a second, so ask again before counting the
/// lookup as failed and waiting for the next refresh.
const FETCH_ATTEMPTS: usize = 3;
/// Wait before the second try; doubled before the third.
const FETCH_BACKOFF: Duration = Duration::from_millis(250);
/// Start of the errors for an answer the API did give (4xx, unreadable body).
const ANSWERED: &str = "Spaces API entitlements";
/// Least time between writes of the plan snapshot.
const PERSIST_EVERY: Duration = Duration::from_secs(10);

/// Where the meter's plans come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlansSource {
    /// The plans file (`CUA_RELAY_PLANS_FILE`), or everyone on the default
    /// plan without one.
    #[default]
    File,
    /// The Spaces API's entitlements.
    Entitlements,
}

impl PlansSource {
    /// `file` / `entitlements`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "file" => Ok(Self::File),
            "entitlements" | "api" => Ok(Self::Entitlements),
            other => Err(format!("{other:?}: use file or entitlements")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Entitlements => "entitlements",
        }
    }
}

/// The fields of `/internal/v1/entitlements/{sub}` the relay reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntitlementsReply {
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub orgs: Vec<OrgReply>,
}

/// One organization of the person.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrgReply {
    pub id: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub seats: i64,
}

/// A subscription status that still pays (grace keeps the plan, like the
/// API's own features).
fn paying(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "active" | "trialing" | "grace"
    )
}

/// The meter's view of an entitlements answer.
pub fn assignment_from(reply: &EntitlementsReply) -> Assignment {
    let mut orgs: Vec<&OrgReply> = reply
        .orgs
        .iter()
        .filter(|o| !o.id.is_empty() && o.plan.eq_ignore_ascii_case("teams") && paying(&o.status))
        .collect();
    // Deterministic when someone is in two paying teams.
    orgs.sort_by(|a, b| a.id.cmp(&b.id));
    let team = orgs.first().map(|o| TeamPlan {
        id: o.id.clone(),
        plan: "teams".into(),
        seats: u32::try_from(o.seats).unwrap_or(0).max(1),
    });
    let plan = reply.plan.trim().to_ascii_lowercase();
    Assignment {
        plan: (!plan.is_empty()).then_some(plan),
        team,
    }
}

#[derive(Debug, Clone)]
struct Cached {
    /// `None` while no answer ever arrived (metered on the default plan).
    assignment: Option<Assignment>,
    /// When to ask again.
    refresh_at: Instant,
    /// Last use, for eviction.
    used: Instant,
}

/// Counters exposed on `/metrics`.
#[derive(Debug, Default)]
pub struct PlanCounters {
    pub lookups: AtomicU64,
    pub errors: AtomicU64,
    pub cache_hits: AtomicU64,
    pub stale_served: AtomicU64,
    pub unknown_served: AtomicU64,
    pub changes: AtomicU64,
    /// Lookups that failed for a user never answered for (metered as free
    /// until the retry succeeds).
    pub unknown_errors: AtomicU64,
    /// Plans loaded from the snapshot at start.
    pub restored: AtomicU64,
}

type OnChange = Box<dyn Fn(&str) + Send + Sync>;

struct Inner {
    org: Arc<OrgAccess>,
    ttl: Duration,
    cache: Mutex<HashMap<String, Cached>>,
    inflight: Mutex<HashSet<String>>,
    on_change: OnceLock<OnChange>,
    counters: PlanCounters,
    permits: tokio::sync::Semaphore,
    /// Last-known plans survive a restart here (the relay's state volume).
    snapshot: Option<std::path::PathBuf>,
    last_persist: Mutex<Option<Instant>>,
}

/// Plans from the Spaces API's entitlements, cached (see the module docs).
#[derive(Clone)]
pub struct EntitlementPlans {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EntitlementPlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntitlementPlans")
            .field("ttl", &self.inner.ttl)
            .finish_non_exhaustive()
    }
}

impl EntitlementPlans {
    /// A source asking the Spaces API `org` is configured for, caching each
    /// answer for `ttl` (default [`DEFAULT_TTL`]).
    pub fn new(org: Arc<OrgAccess>, ttl: Option<Duration>) -> Self {
        Self::with_snapshot(org, ttl, None)
    }

    /// Like [`Self::new`], keeping the last known plans in `snapshot` so a
    /// restart does not meter a payer as free while the API is slow or down.
    /// Loaded plans are refreshed at once; they only stand in until then.
    pub fn with_snapshot(
        org: Arc<OrgAccess>,
        ttl: Option<Duration>,
        snapshot: Option<std::path::PathBuf>,
    ) -> Self {
        let counters = PlanCounters::default();
        let mut cache = HashMap::new();
        if let Some(path) = &snapshot {
            match std::fs::read(path) {
                Ok(raw) => match serde_json::from_slice::<HashMap<String, Assignment>>(&raw) {
                    Ok(saved) => {
                        let now = Instant::now();
                        for (user, a) in saved.into_iter().take(MAX_ENTRIES) {
                            cache.insert(
                                user,
                                Cached {
                                    assignment: Some(a),
                                    refresh_at: now,
                                    used: now,
                                },
                            );
                        }
                        counters
                            .restored
                            .store(cache.len() as u64, Ordering::Relaxed);
                        tracing::info!(plans = cache.len(), "relay plans restored from snapshot");
                    }
                    Err(e) => tracing::warn!(error = %e, "relay plan snapshot unreadable; ignored"),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(error = %e, "relay plan snapshot not read"),
            }
        }
        Self {
            inner: Arc::new(Inner {
                org,
                ttl: ttl.unwrap_or(DEFAULT_TTL),
                cache: Mutex::new(cache),
                inflight: Mutex::default(),
                on_change: OnceLock::new(),
                counters,
                permits: tokio::sync::Semaphore::new(MAX_CONCURRENT_LOOKUPS),
                snapshot,
                last_persist: Mutex::new(None),
            }),
        }
    }

    /// Account deletion: drops `user` from the cache and rewrites the
    /// snapshot without them (now, not throttled). True when they were
    /// cached. Idempotent; the snapshot is rewritten every call.
    pub fn forget(&self, user: &str) -> std::io::Result<bool> {
        let existed = self
            .inner
            .cache
            .lock()
            .expect("plan cache")
            .remove(user)
            .is_some();
        self.inner.write_snapshot().map_err(std::io::Error::other)?;
        Ok(existed)
    }

    /// Called with a user's `sub` when a new answer changes their plan or
    /// team (the meter re-resolves them). Set once.
    pub fn set_on_change(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        let _ = self.inner.on_change.set(Box::new(f));
    }

    pub fn counters(&self) -> &PlanCounters {
        &self.inner.counters
    }

    /// Asks the API now unless a fresh answer is cached, waiting at most
    /// `timeout` (for `/v1/usage`, so the first read after a purchase or a
    /// restart shows the real plan). Errors keep the cached answer.
    pub async fn warm(&self, user: &str, timeout: Duration) {
        let fresh = self
            .inner
            .cache
            .lock()
            .expect("plan cache")
            .get(user)
            .is_some_and(|c| c.assignment.is_some() && Instant::now() < c.refresh_at);
        if !fresh {
            let _ =
                tokio::time::timeout(timeout, self.inner.clone().refresh(user.to_owned())).await;
        }
    }

    /// Prometheus lines (counts only, no account data).
    pub fn metrics(&self) -> String {
        let c = &self.inner.counters;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut by_plan: std::collections::BTreeMap<String, u64> = Default::default();
        let mut teams = HashSet::new();
        for e in self.inner.cache.lock().expect("plan cache").values() {
            let (plan, team) = match &e.assignment {
                Some(a) => (
                    a.team
                        .as_ref()
                        .map(|t| t.plan.clone())
                        .or_else(|| a.plan.clone())
                        .unwrap_or_else(|| DEFAULT_PLAN.into()),
                    a.team.as_ref().map(|t| t.id.clone()),
                ),
                None => ("unknown".into(), None),
            };
            *by_plan.entry(plan).or_default() += 1;
            if let Some(t) = team {
                teams.insert(t);
            }
        }
        let mut out = format!(
            "# TYPE cua_relay_plans_source gauge\ncua_relay_plans_source{{source=\"entitlements\"}} 1\n\
             # TYPE cua_relay_plans_lookups_total counter\ncua_relay_plans_lookups_total {}\n\
             # TYPE cua_relay_plans_errors_total counter\ncua_relay_plans_errors_total {}\n\
             # TYPE cua_relay_plans_cache_hits_total counter\ncua_relay_plans_cache_hits_total {}\n\
             # TYPE cua_relay_plans_stale_served_total counter\ncua_relay_plans_stale_served_total {}\n\
             # TYPE cua_relay_plans_unknown_served_total counter\ncua_relay_plans_unknown_served_total {}\n\
             # TYPE cua_relay_plans_changes_total counter\ncua_relay_plans_changes_total {}\n\
             # TYPE cua_relay_plans_unknown_errors_total counter\ncua_relay_plans_unknown_errors_total {}\n\
             # TYPE cua_relay_plans_restored gauge\ncua_relay_plans_restored {}\n\
             # TYPE cua_relay_plans_teams_cached gauge\ncua_relay_plans_teams_cached {}\n\
             # TYPE cua_relay_plans_users_cached gauge\n",
            get(&c.lookups),
            get(&c.errors),
            get(&c.cache_hits),
            get(&c.stale_served),
            get(&c.unknown_served),
            get(&c.changes),
            get(&c.unknown_errors),
            get(&c.restored),
            teams.len(),
        );
        for (plan, n) in by_plan {
            out.push_str(&format!(
                "cua_relay_plans_users_cached{{plan=\"{plan}\"}} {n}\n"
            ));
        }
        out
    }

    /// Starts a background lookup of `user` (one at a time per user).
    fn spawn_refresh(&self, user: &str) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = self.inner.clone();
        let user = user.to_owned();
        rt.spawn(async move { inner.refresh(user).await });
    }
}

impl Inner {
    async fn refresh(self: Arc<Self>, user: String) {
        if !self
            .inflight
            .lock()
            .expect("plan inflight")
            .insert(user.clone())
        {
            return;
        }
        struct Done<'a>(&'a Inner, &'a str);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0
                    .inflight
                    .lock()
                    .expect("plan inflight")
                    .remove(self.1);
            }
        }
        let _done = Done(&self, &user);
        self.counters.lookups.fetch_add(1, Ordering::Relaxed);
        let answer = self.fetch_with_retry(&user).await;
        let now = Instant::now();
        let changed = {
            let mut cache = self.cache.lock().expect("plan cache");
            if cache.len() >= MAX_ENTRIES && !cache.contains_key(&user) {
                let cutoff = now.checked_sub(self.ttl * 10).unwrap_or(now);
                cache.retain(|_, c| c.used >= cutoff);
                if cache.len() >= MAX_ENTRIES {
                    cache.clear();
                }
            }
            let entry = cache.entry(user.clone()).or_insert(Cached {
                assignment: None,
                refresh_at: now,
                used: now,
            });
            match answer {
                Ok(a) => {
                    let changed = entry.assignment.as_ref() != Some(&a);
                    entry.assignment = Some(a);
                    entry.refresh_at = now + self.ttl;
                    changed
                }
                Err(e) => {
                    self.counters.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(error = %e, known = entry.assignment.is_some(),
                        "relay plan lookup failed; keeping the last known plan");
                    let known = entry.assignment.is_some();
                    if !known {
                        self.counters.unknown_errors.fetch_add(1, Ordering::Relaxed);
                    }
                    let wait = if known {
                        RETRY_AFTER
                    } else {
                        RETRY_UNKNOWN_AFTER
                    };
                    entry.refresh_at = now + wait.min(self.ttl);
                    false
                }
            }
        };
        if changed {
            self.persist();
            self.counters.changes.fetch_add(1, Ordering::Relaxed);
            if let Some(f) = self.on_change.get() {
                f(&user);
            }
        }
    }

    /// Writes the known plans to the snapshot, at most every
    /// [`PERSIST_EVERY`]. A failed write is logged, never fatal.
    fn persist(&self) {
        if self.snapshot.is_none() {
            return;
        }
        {
            let mut last = self.last_persist.lock().expect("plan persist");
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < PERSIST_EVERY) {
                return;
            }
            *last = Some(now);
        }
        if let Err(e) = self.write_snapshot() {
            tracing::warn!(error = %e, "relay plan snapshot not written");
        }
    }

    /// Writes the snapshot now (nothing without one).
    fn write_snapshot(&self) -> Result<(), String> {
        let Some(path) = &self.snapshot else {
            return Ok(());
        };
        let known: HashMap<String, Assignment> = self
            .cache
            .lock()
            .expect("plan cache")
            .iter()
            .filter_map(|(u, c)| c.assignment.clone().map(|a| (u.clone(), a)))
            .collect();
        let tmp = path.with_extension("tmp");
        serde_json::to_vec(&known)
            .map_err(|e| e.to_string())
            .and_then(|raw| std::fs::write(&tmp, raw).map_err(|e| e.to_string()))
            .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| e.to_string()))
    }

    /// [`Self::fetch`], asked again with a short backoff after a transport
    /// error, a 5xx or a refused service token. The permit is held per try,
    /// never while waiting, so a retry cannot starve other users' lookups.
    async fn fetch_with_retry(&self, user: &str) -> Result<Assignment, String> {
        let mut wait = FETCH_BACKOFF;
        let mut last = Err("no lookup made".to_owned());
        for attempt in 1..=FETCH_ATTEMPTS {
            last = {
                let _permit = self.permits.acquire().await;
                self.fetch(user).await
            };
            // Only a transport error, a 5xx or a refused token is worth asking
            // again; a 4xx (429, 403) or an unreadable answer is final.
            let final_answer = matches!(&last, Err(e) if e.starts_with(ANSWERED));
            if last.is_ok() || final_answer || attempt == FETCH_ATTEMPTS || !self.org.has_api() {
                break;
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        }
        last
    }

    async fn fetch(&self, user: &str) -> Result<Assignment, String> {
        let path = format!(
            "/internal/v1/entitlements/{}",
            url::form_urlencoded::byte_serialize(user.as_bytes()).collect::<String>()
        );
        let r = self.org.get(&path, None).await?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Assignment::default());
        }
        if !r.status().is_success() {
            return Err(format!("{ANSWERED}: HTTP {}", r.status()));
        }
        let reply: EntitlementsReply = r
            .json()
            .await
            .map_err(|e| format!("{ANSWERED} answer: {}", e.without_url()))?;
        Ok(assignment_from(&reply))
    }
}

impl PlanSource for EntitlementPlans {
    fn assignment(&self, user: &str, _email: Option<&str>, _: &[Membership]) -> Assignment {
        let now = Instant::now();
        let (known, refresh) = {
            let mut cache = self.inner.cache.lock().expect("plan cache");
            match cache.get_mut(user) {
                Some(c) => {
                    c.used = now;
                    (c.assignment.clone(), now >= c.refresh_at)
                }
                None => (None, true),
            }
        };
        let c = &self.inner.counters;
        match (&known, refresh) {
            (Some(_), false) => c.cache_hits.fetch_add(1, Ordering::Relaxed),
            (Some(_), true) => c.stale_served.fetch_add(1, Ordering::Relaxed),
            (None, _) => c.unknown_served.fetch_add(1, Ordering::Relaxed),
        };
        if refresh {
            self.spawn_refresh(user);
        }
        known.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(v: serde_json::Value) -> EntitlementsReply {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn parses_sources() {
        assert_eq!(PlansSource::parse("").unwrap(), PlansSource::File);
        assert_eq!(
            PlansSource::parse("Entitlements").unwrap(),
            PlansSource::Entitlements
        );
        assert!(PlansSource::parse("stripe").is_err());
    }

    #[test]
    fn maps_free_pro_and_teams() {
        let free = assignment_from(&reply(json!({"plan": "free", "orgs": []})));
        assert_eq!(free.plan.as_deref(), Some("free"));
        assert!(free.team.is_none());

        let pro = assignment_from(&reply(json!({"plan": "Pro", "status": "active"})));
        assert_eq!(pro.plan.as_deref(), Some("pro"));
        assert!(pro.team.is_none());

        let teams = assignment_from(&reply(json!({
            "plan": "teams",
            "orgs": [
                {"id": "o-unpaid", "plan": "free", "status": "none", "seats": 0},
                {"id": "o-b", "plan": "teams", "status": "active", "seats": 4},
                {"id": "o-a", "plan": "teams", "status": "grace", "seats": 2},
                {"id": "o-cancelled", "plan": "teams", "status": "none", "seats": 9},
            ],
        })));
        assert_eq!(teams.plan.as_deref(), Some("teams"));
        let t = teams.team.unwrap();
        assert_eq!(
            (t.id.as_str(), t.plan.as_str(), t.seats),
            ("o-a", "teams", 2)
        );

        // Zero seats still pools one allowance; no plan means the default.
        let zero = assignment_from(&reply(json!({
            "orgs": [{"id": "o", "plan": "teams", "status": "trialing", "seats": 0}],
        })));
        assert_eq!(zero.plan, None);
        assert_eq!(zero.team.unwrap().seats, 1);
    }

    #[tokio::test]
    async fn unknown_is_default_and_errors_keep_the_last_plan() {
        let plans = EntitlementPlans::new(Arc::new(OrgAccess::default()), None);
        // No API: unknown users are on the default plan, never refused.
        assert_eq!(plans.assignment("ada", None, &[]), Assignment::default());
        plans.warm("ada", Duration::from_secs(1)).await;
        assert_eq!(plans.assignment("ada", None, &[]), Assignment::default());
        assert!(plans.counters().errors.load(Ordering::Relaxed) >= 1);

        // A known plan survives a failed refresh.
        let pro = Assignment {
            plan: Some("pro".into()),
            team: None,
        };
        plans.inner.cache.lock().unwrap().insert(
            "bob".into(),
            Cached {
                assignment: Some(pro.clone()),
                refresh_at: Instant::now(),
                used: Instant::now(),
            },
        );
        plans.warm("bob", Duration::from_secs(1)).await;
        assert_eq!(plans.assignment("bob", None, &[]), pro);
        let m = plans.metrics();
        assert!(
            m.contains("cua_relay_plans_users_cached{plan=\"pro\"} 1"),
            "{m}"
        );
    }

    #[tokio::test]
    async fn a_failed_lookup_never_names_the_account_in_its_error() {
        // The entitlements URL carries the account id; a connection error
        // must not repeat it (it ends up in a WARN log line).
        let org = Arc::new(OrgAccess::new(crate::org_access::OrgAccessConfig {
            api_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        }));
        let plans = EntitlementPlans::new(org, None);
        let err = plans.inner.fetch("acct-5f0c-secret-sub").await.unwrap_err();
        assert!(err.starts_with("Spaces API"), "{err}");
        assert!(!err.contains("acct-5f0c-secret-sub"), "{err}");
        assert!(!err.contains("/internal/v1/entitlements"), "{err}");
    }

    #[tokio::test]
    async fn a_lookup_is_asked_again_after_a_transport_error() {
        // A server that drops the first connection, then answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (first, _) = listener.accept().await.unwrap();
            drop(first);
            loop {
                let (mut c, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf).await;
                let body = r#"{"orgs":[]}"#;
                let _ = c
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            }
        });
        let org = OrgAccess::new(crate::org_access::OrgAccessConfig {
            api_url: Some(format!("http://{addr}")),
            ..Default::default()
        });
        let plans = EntitlementPlans::new(Arc::new(org), None);
        let got = plans.inner.fetch_with_retry("ada").await;
        assert!(got.is_ok(), "{got:?}");
    }

    #[tokio::test]
    async fn a_refused_lookup_is_not_asked_again() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicU64::new(0));
        let seen = hits.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut c, _) = listener.accept().await.unwrap();
                seen.fetch_add(1, Ordering::Relaxed);
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf).await;
                let _ = c
                    .write_all(
                        b"HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
            }
        });
        let org = OrgAccess::new(crate::org_access::OrgAccessConfig {
            api_url: Some(format!("http://{addr}")),
            ..Default::default()
        });
        let plans = EntitlementPlans::new(Arc::new(org), None);
        assert!(plans.inner.fetch_with_retry("ada").await.is_err());
        assert_eq!(hits.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn snapshot_restores_a_payer_when_the_api_is_down() {
        let dir = std::env::temp_dir().join(format!("plans-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("relay.plans.json");
        let saved: HashMap<String, Assignment> = HashMap::from([(
            "u1".to_owned(),
            Assignment {
                plan: Some("pro".into()),
                team: None,
            },
        )]);
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        // No API configured: every lookup fails.
        let org = Arc::new(OrgAccess::new(Default::default()));
        let plans = EntitlementPlans::with_snapshot(org, None, Some(path.clone()));
        let a = plans.assignment("u1", None, &[]);
        assert_eq!(a.plan.as_deref(), Some("pro"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let a = plans.assignment("u1", None, &[]);
        assert_eq!(a.plan.as_deref(), Some("pro"), "an error keeps the payer");
        assert_eq!(plans.counters().restored.load(Ordering::Relaxed), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
