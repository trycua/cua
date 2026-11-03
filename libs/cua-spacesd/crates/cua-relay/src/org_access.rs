// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Team access from Keycloak Organizations and the Spaces API (CUA-1481).
//!
//! Who is in which team, and with which role, comes from the account
//! token's `organization` claim. Which machines belong to a team and which
//! member devices an admin approved come from the Spaces API's internal
//! routes, called with the relay's own service token:
//!
//! - `GET /internal/v1/access?sub&org&machine&device` -> `{member, role,
//!   machine_in_org, device_approved, ttl}`
//! - `GET /internal/v1/orgs/{org}/machines` (ETag) -> the team's machines
//!
//! Answers (yes and no) are cached for a short TTL. When the API is
//! unreachable (network error, timeout, 5xx) a previous "allowed" keeps being
//! served for up to 15 minutes since it was last confirmed (counted in
//! `cua_relay_org_stale_served_total`); an explicit "no" always wins at once,
//! and anyone never allowed (cache miss) is refused (fail closed). Personal
//! machines never depend on the API. The API pushes `POST /internal/v1/membership-
//! changed {org, sub?}` when a member is removed or their role changes: the
//! relay drops what it cached for them and re-checks their open sessions.
//!
//! [`TeamsSource`] picks where team access comes from: `file` (the relay's
//! own `teams.json`, the default), `shadow` (still `teams.json`, while the
//! organization path is evaluated beside it and every disagreement is
//! logged), or `org` (this module only; `/v1/teams/*` answers 410).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::oidc::OrgClaim;

/// Where team access comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TeamsSource {
    /// `teams.json` only.
    #[default]
    File,
    /// `teams.json` decides; the organization path is evaluated and
    /// mismatches are logged.
    Shadow,
    /// Organizations and the Spaces API only.
    Org,
}

impl TeamsSource {
    /// `file` / `shadow` / `org`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "file" => Ok(Self::File),
            "shadow" => Ok(Self::Shadow),
            "org" => Ok(Self::Org),
            other => Err(format!("{other:?}: use file, shadow or org")),
        }
    }

    /// The organization path is evaluated.
    pub fn evaluates_orgs(self) -> bool {
        !matches!(self, Self::File)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Shadow => "shadow",
            Self::Org => "org",
        }
    }
}

/// Default cache lifetime of an access answer.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);
/// How long a previous "allowed" answer keeps being served while the Spaces
/// API (or Keycloak behind it) cannot be reached, counted from when it was
/// last confirmed. Only network errors, timeouts and 5xx count as an outage:
/// an explicit "no" is always honoured at once, and a caller never allowed
/// before fails closed straight away.
pub const DEFAULT_STALE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Most cached access answers kept (expired ones are swept first).
const MAX_ENTRIES: usize = 50_000;
/// How often the clean-up of outside shares runs.
pub const SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Start of the error for a 401 the Spaces API gave a freshly minted
/// service token: a configuration problem, never worth asking again.
pub const TOKEN_REFUSED: &str = "Spaces API refused the relay's service token";

/// How long a service token is used before a new one is minted: half its
/// lifetime (`expires_in`, 60 s when absent), at least one second.
///
/// The relay cannot see the issuer's or the API's clock, only the
/// lifetime. Keycloak stamps `exp` from its own clock and the API checks it
/// against its own (30 s leeway), so a token used until shortly before its
/// nominal end is refused whenever the issuer runs behind the API by more
/// than the remaining margin plus the leeway (CUA-2959: a node 60 s slow,
/// refusals in the last second of every 270 s token). Half the lifetime
/// (150 s for the relay's 300 s tokens) leaves room for minutes of skew,
/// the usual OAuth client practice; a refusal past that is still
/// recovered by [`OrgAccess::send_authed`].
pub fn token_refresh_after(expires_in: Option<u64>) -> Duration {
    Duration::from_millis(expires_in.unwrap_or(60).saturating_mul(500).max(1000))
}

/// The clock cache expiry reads (see [`crate::clock::Clock`]).
pub use crate::clock::Clock;

/// Settings.
#[derive(Debug, Clone, Default)]
pub struct OrgAccessConfig {
    /// Cache clock; only tests advance it.
    pub clock: Clock,
    pub source: TeamsSource,
    /// Spaces API base URL (set from configuration, e.g. an in-cluster
    /// service URL).
    pub api_url: Option<String>,
    /// Token endpoint for the relay's client-credentials token.
    pub token_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    /// Lifetime of a cached answer (default [`DEFAULT_TTL`]).
    pub ttl: Option<Duration>,
    /// How long a last-known "allowed" is served during an outage (default
    /// [`DEFAULT_STALE_WINDOW`]).
    pub stale_window: Option<Duration>,
    /// Bearer token the Spaces API presents on the relay's
    /// `/internal/v1/*` routes.
    pub internal_token: Option<String>,
    /// Where the meter's plans come from (the Spaces API's entitlements
    /// need `api_url`).
    pub plans_source: crate::plan_source::PlansSource,
    /// Share computers only with people in the owner's team (CUA-1883).
    /// Off by default: the relay then behaves as before.
    pub team_only_sharing: bool,
    /// How often the clean-up of outside shares runs (default daily; it
    /// also runs once on start).
    pub sweep_every: Option<Duration>,
}

/// One answer of `/internal/v1/access`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct AccessAnswer {
    #[serde(default)]
    pub member: bool,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub machine_in_org: bool,
    /// What the team may do with the computer: `viewer` (watch only) or
    /// `editor`. Absent (an older Spaces API) means `editor`.
    #[serde(default)]
    pub machine_role: Option<String>,
    #[serde(default)]
    pub device_approved: bool,
    /// Seconds the answer may be cached (capped at the configured TTL).
    #[serde(default)]
    pub ttl: Option<u64>,
}

impl AccessAnswer {
    /// The team's role on the computer is watch only.
    pub fn viewer_only(&self) -> bool {
        self.machine_role
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("viewer"))
    }
}

/// What the organization path says about one caller and machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgDecision {
    /// Reachable through organization `org`.
    Granted { org: String },
    /// In an organization the machine belongs to, but this device is not
    /// approved (or no device was presented).
    DeviceRefused { org: String, message: String },
    /// Not reachable through any organization in the token.
    NotShared,
    /// The Spaces API could not be asked and nothing was cached.
    Unavailable(String),
}

impl OrgDecision {
    pub fn granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }

    /// A short label for logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::DeviceRefused { .. } => "device_refused",
            Self::NotShared => "not_shared",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    answer: AccessAnswer,
    at: Instant,
    ttl: Duration,
}

impl Entry {
    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < self.ttl
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    sub: String,
    org: String,
    machine: String,
    device: String,
}

/// The Spaces API's answer about one person on one owner's computers.
#[derive(Debug, Clone)]
struct ShareEntry {
    allowed: bool,
    /// The owner is in no team that can share.
    no_team: bool,
    at: Instant,
    ttl: Duration,
}

impl ShareEntry {
    fn fresh(&self) -> bool {
        self.at.elapsed() < self.ttl
    }
}

/// Why sharing is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareRefusal {
    /// The owner is in no team that can share (`sharing_needs_team`).
    NeedsTeam,
    /// These people are not in the owner's team (`share_outside_team`).
    OutsideTeam(Vec<String>),
}

#[derive(Debug, Clone)]
struct MachineList {
    machines: Vec<String>,
    etag: Option<String>,
    at: Instant,
}

/// Counters exposed on `/metrics`.
#[derive(Debug, Default)]
pub struct OrgCounters {
    pub api_calls: AtomicU64,
    pub api_errors: AtomicU64,
    pub cache_hits: AtomicU64,
    pub shadow_checks: AtomicU64,
    pub shadow_mismatches: AtomicU64,
    pub fail_closed: AtomicU64,
    /// Answers served from the last-known-good cache during an outage.
    pub stale_served: AtomicU64,
    pub membership_pushes: AtomicU64,
    /// Service tokens minted at the token endpoint.
    pub token_fetches: AtomicU64,
    /// Mints that failed (token endpoint down, secret refused).
    pub token_errors: AtomicU64,
    /// 401s to a cached service token, answered by minting a new one and
    /// asking again (not API errors; see [`OrgAccess::send_authed`]).
    pub token_refused: AtomicU64,
}

/// The organization access path: API client plus caches.
#[derive(Debug)]
pub struct OrgAccess {
    config: OrgAccessConfig,
    http: reqwest::Client,
    token: tokio::sync::Mutex<Option<(String, Instant)>>,
    entries: Mutex<HashMap<Key, Entry>>,
    machines: Mutex<HashMap<String, MachineList>>,
    /// (owner, person) -> may the owner share with them.
    shares: Mutex<HashMap<(String, String), ShareEntry>>,
    /// One share-check in flight per owner: callers that miss together
    /// wait for the first answer instead of each asking.
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Last shadow-mismatch log line per (what, sub, machine, outcome).
    logged: Mutex<HashMap<String, Instant>>,
    pub counters: OrgCounters,
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl Default for OrgAccess {
    fn default() -> Self {
        Self::new(OrgAccessConfig::default())
    }
}

impl OrgAccess {
    pub fn new(config: OrgAccessConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_default();
        Self {
            config,
            http,
            token: tokio::sync::Mutex::new(None),
            entries: Mutex::default(),
            machines: Mutex::default(),
            shares: Mutex::default(),
            flights: Mutex::default(),
            logged: Mutex::default(),
            counters: OrgCounters::default(),
        }
    }

    pub fn source(&self) -> TeamsSource {
        self.config.source
    }

    pub fn internal_token(&self) -> Option<&str> {
        self.config.internal_token.as_deref()
    }

    fn ttl(&self) -> Duration {
        self.config.ttl.unwrap_or(DEFAULT_TTL)
    }

    fn stale_window(&self) -> Duration {
        self.config.stale_window.unwrap_or(DEFAULT_STALE_WINDOW)
    }

    /// Counts a stale answer and logs it (at most once a minute per `what`
    /// and key), so L4 or the canary can alert on an outage being papered
    /// over.
    fn note_stale(&self, what: &str, key: &str, age: Duration, detail: &str) {
        self.counters.stale_served.fetch_add(1, Ordering::Relaxed);
        let log_key = format!("stale:{what}:{key}");
        let mut logged = self.logged.lock().expect("shadow log");
        if logged
            .get(&log_key)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        if logged.len() > 10_000 {
            logged.retain(|_, t| t.elapsed() < Duration::from_secs(60));
        }
        logged.insert(log_key, Instant::now());
        tracing::warn!(
            target: "cua_relay::org_stale",
            what,
            key,
            age_secs = age.as_secs(),
            detail,
            "Spaces API unreachable: serving the last known \"allowed\" answer"
        );
    }

    /// The last "allowed" answer for `key`, if it was confirmed within the
    /// stale window. Anything older is dropped.
    fn stale_allowed(&self, key: &Key, detail: &str) -> Option<AccessAnswer> {
        let mut entries = self.entries.lock().expect("org entries");
        let e = entries.get(key)?;
        let age = self.config.clock.now().saturating_duration_since(e.at);
        if e.answer.member && age < self.stale_window() {
            let answer = e.answer.clone();
            drop(entries);
            self.note_stale(
                "access",
                &format!("{}:{}:{}", key.sub, key.org, key.machine),
                age,
                detail,
            );
            return Some(answer);
        }
        entries.remove(key);
        None
    }

    /// The relay's service token (client credentials), cached and minted
    /// again at half its lifetime (see [`token_refresh_after`]). `None`
    /// without a token endpoint or secret (a test API without auth).
    async fn service_token(&self) -> Result<Option<String>, String> {
        self.service_token_replacing(None).await
    }

    /// Like [`Self::service_token`], but when the cached token is
    /// `refused` (the API just answered 401 to it), mints a new one. Callers
    /// that were refused together mint once: whoever comes second finds a
    /// different, fresh token already cached and uses it.
    async fn service_token_replacing(
        &self,
        refused: Option<&str>,
    ) -> Result<Option<String>, String> {
        let (Some(url), Some(secret)) = (&self.config.token_url, &self.config.client_secret) else {
            return Ok(None);
        };
        let mut cached = self.token.lock().await;
        if let Some((token, refresh_at)) = cached.as_ref() {
            if Instant::now() < *refresh_at && refused != Some(token.as_str()) {
                return Ok(Some(token.clone()));
            }
        }
        *cached = None;
        self.counters.token_fetches.fetch_add(1, Ordering::Relaxed);
        let minted = async {
            let reply = self
                .http
                .post(url)
                .form(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", self.config.client_id.as_str()),
                    ("client_secret", secret.as_str()),
                ])
                .send()
                .await
                .map_err(|e| format!("service token: {}", error_chain(&e.without_url())))?;
            if !reply.status().is_success() {
                return Err(format!("service token: HTTP {}", reply.status()));
            }
            reply
                .json::<TokenReply>()
                .await
                .map_err(|e| format!("service token: {}", e.without_url()))
        }
        .await;
        let t = minted.inspect_err(|_| {
            self.counters.token_errors.fetch_add(1, Ordering::Relaxed);
        })?;
        *cached = Some((
            t.access_token.clone(),
            Instant::now() + token_refresh_after(t.expires_in),
        ));
        Ok(Some(t.access_token))
    }

    /// Sends the request `build` makes with the service token. A 401 means
    /// the API refused that token: it is minted again and the request sent
    /// once more, so a token the API considers expired before the relay
    /// does (the issuer's clock behind the API's, CUA-2959) costs one extra
    /// round trip, not an error. Only a 401 to a freshly minted token is
    /// returned as such ([`TOKEN_REFUSED`]): that is a configuration
    /// problem (client secret, `SPACES_CP_SERVICE_CLIENTS`), not something
    /// to retry.
    async fn send_authed(
        &self,
        build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        let mut token = self.service_token().await?;
        let mut refreshed = false;
        loop {
            let mut req = build(&self.http);
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            self.counters.api_calls.fetch_add(1, Ordering::Relaxed);
            let r = req.send().await.map_err(|e| {
                self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
                format!("Spaces API: {}", error_chain(&e.without_url()))
            })?;
            if r.status() != reqwest::StatusCode::UNAUTHORIZED {
                return Ok(r);
            }
            let Some(used) = token.as_deref() else {
                // No service token configured: nothing to refresh.
                self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
                return Err(format!(
                    "{TOKEN_REFUSED} (HTTP 401, no service token configured)"
                ));
            };
            if refreshed {
                self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
                return Err(format!("{TOKEN_REFUSED} (HTTP 401 to a fresh token)"));
            }
            self.counters.token_refused.fetch_add(1, Ordering::Relaxed);
            token = self.service_token_replacing(Some(used)).await?;
            refreshed = true;
        }
    }

    pub(crate) async fn get(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        let base = self
            .config
            .api_url
            .as_deref()
            .ok_or_else(|| "no Spaces API configured".to_owned())?;
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let r = self
            .send_authed(|http| {
                let req = http.get(&url);
                match etag {
                    Some(e) => req.header(reqwest::header::IF_NONE_MATCH, e),
                    None => req,
                }
            })
            .await?;
        if r.status().is_server_error() {
            self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
            return Err(format!("Spaces API: HTTP {}", r.status()));
        }
        Ok(r)
    }

    /// Whether a Spaces API is configured at all.
    pub fn has_api(&self) -> bool {
        self.config.api_url.is_some()
    }

    /// POSTs `body` as JSON to the Spaces API and returns the status.
    pub(crate) async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::StatusCode, String> {
        let base = self
            .config
            .api_url
            .as_deref()
            .ok_or_else(|| "no Spaces API configured".to_owned())?;
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let r = self.send_authed(|http| http.post(&url).json(body)).await?;
        if !r.status().is_success() {
            self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
        }
        Ok(r.status())
    }

    /// POSTs `body` as JSON and returns the status and JSON reply. A server
    /// error or a refused service token is an error, like [`Self::get`].
    pub(crate) async fn post_json_reply(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(reqwest::StatusCode, serde_json::Value), String> {
        let base = self
            .config
            .api_url
            .as_deref()
            .ok_or_else(|| "no Spaces API configured".to_owned())?;
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let r = self.send_authed(|http| http.post(&url).json(body)).await?;
        let status = r.status();
        if status.is_server_error() {
            self.counters.api_errors.fetch_add(1, Ordering::Relaxed);
            return Err(format!("Spaces API: HTTP {status}"));
        }
        let value = r
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("Spaces API reply: {}", e.without_url()))?;
        Ok((status, value))
    }

    /// Whether sharing is limited to the owner's team.
    pub fn team_only_sharing(&self) -> bool {
        self.config.team_only_sharing
    }

    /// How often the clean-up of outside shares runs.
    pub fn sweep_every(&self) -> Duration {
        self.config.sweep_every.unwrap_or(SWEEP_EVERY)
    }

    /// May `owner` share with `who` (account ids or emails)? Asks
    /// `/internal/v1/share-check` for the people not cached (60 s, like the
    /// access answer). The relay stores no teams: it only keeps the answer.
    ///
    /// One call per owner at a time (single flight), and one call covers
    /// everyone missing. Fails closed: with the Spaces API down, a person
    /// with no fresh answer is an error; an answer older than the TTL is
    /// not used.
    pub async fn share_check(
        &self,
        owner: &str,
        who: &[String],
    ) -> Result<Result<(), ShareRefusal>, String> {
        let key = |w: &String| (owner.to_owned(), w.to_ascii_lowercase());
        let missing_now = || -> Vec<String> {
            let shares = self.shares.lock().expect("share entries");
            who.iter()
                .filter(|w| shares.get(&key(w)).is_none_or(|e| !e.fresh()))
                .cloned()
                .collect()
        };
        let mut missing = missing_now();
        let _flight = if missing.is_empty() {
            None
        } else {
            let lock = {
                let mut flights = self.flights.lock().expect("share flights");
                if flights.len() > 10_000 {
                    flights.retain(|_, l| Arc::strong_count(l) > 1);
                }
                flights.entry(owner.to_owned()).or_default().clone()
            };
            let guard = lock.lock_owned().await;
            // Whoever held the lock may have answered already.
            missing = missing_now();
            Some(guard)
        };
        if !missing.is_empty() {
            let body = serde_json::json!({"owner_sub": owner, "who": missing});
            match self
                .post_json_reply("/internal/v1/share-check", &body)
                .await
            {
                Ok((status, reply)) if status.is_success() => {
                    let ttl = reply
                        .get("ttl")
                        .and_then(|t| t.as_u64())
                        .map(Duration::from_secs)
                        .unwrap_or(self.ttl())
                        .min(self.ttl());
                    let needs_team =
                        reply.get("can_share").and_then(|c| c.as_bool()) == Some(false);
                    let results = reply
                        .get("results")
                        .and_then(|r| r.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let mut shares = self.shares.lock().expect("share entries");
                    if shares.len() >= MAX_ENTRIES {
                        shares.retain(|_, e| e.fresh());
                        if shares.len() >= MAX_ENTRIES {
                            shares.clear();
                        }
                    }
                    for w in &missing {
                        // A person the reply leaves out is refused.
                        let allowed = !needs_team
                            && results.iter().any(|r| {
                                r.get("who").and_then(|x| x.as_str()) == Some(w.as_str())
                                    && r.get("allowed").and_then(|x| x.as_bool()) == Some(true)
                            });
                        shares.insert(
                            key(w),
                            ShareEntry {
                                allowed,
                                no_team: needs_team,
                                at: Instant::now(),
                                ttl,
                            },
                        );
                    }
                }
                Ok((status, _)) => {
                    self.counters.fail_closed.fetch_add(1, Ordering::Relaxed);
                    return Err(format!("Spaces API: HTTP {status}"));
                }
                Err(e) => {
                    // Fail closed: an answer past its TTL is not used.
                    self.counters.fail_closed.fetch_add(1, Ordering::Relaxed);
                    return Err(e);
                }
            }
        }
        let shares = self.shares.lock().expect("share entries");
        let outside: Vec<String> = who
            .iter()
            .filter(|w| !shares.get(&key(w)).is_some_and(|e| e.allowed))
            .cloned()
            .collect();
        if outside.is_empty() {
            Ok(Ok(()))
        } else if outside
            .iter()
            .any(|w| shares.get(&key(w)).is_some_and(|e| e.no_team))
        {
            Ok(Err(ShareRefusal::NeedsTeam))
        } else {
            Ok(Err(ShareRefusal::OutsideTeam(outside)))
        }
    }

    /// From cached answers only: is any of `who` allowed by `owner`'s team?
    /// `None` when nothing is cached for them (the synchronous re-checks
    /// then leave access as it is; opening asks the API first).
    pub fn cached_share_allowed(&self, owner: &str, who: &[String]) -> Option<bool> {
        let shares = self.shares.lock().expect("share entries");
        let mut seen = false;
        for w in who {
            if let Some(e) = shares.get(&(owner.to_owned(), w.to_ascii_lowercase())) {
                if e.allowed {
                    return Some(true);
                }
                seen = true;
            }
        }
        seen.then_some(false)
    }

    /// Account deletion: forgets everything cached for `sub` (and the
    /// `emails` shares were addressed to): access answers, share answers as
    /// the owner or as the person, shadow-log keys, in-flight share checks,
    /// and the erased `machines` in the cached machine lists.
    pub fn erase_account(&self, sub: &str, emails: &[String], machines: &[String]) -> usize {
        let mine = |s: &str| s == sub || emails.iter().any(|e| e.eq_ignore_ascii_case(s));
        let mut n = 0;
        let mut entries = self.entries.lock().expect("org entries");
        let before = entries.len();
        entries.retain(|k, _| k.sub != sub && !machines.contains(&k.machine));
        n += before - entries.len();
        drop(entries);
        let mut shares = self.shares.lock().expect("share entries");
        let before = shares.len();
        shares.retain(|(owner, person), _| !mine(owner) && !mine(person));
        n += before - shares.len();
        drop(shares);
        self.flights
            .lock()
            .expect("flights")
            .retain(|k, _| !mine(k));
        self.logged
            .lock()
            .expect("shadow log")
            .retain(|k, _| !k.contains(sub) && !emails.iter().any(|e| k.contains(e.as_str())));
        for list in self.machines.lock().expect("org machines").values_mut() {
            list.machines.retain(|m| !machines.contains(m));
        }
        n
    }

    /// Forgets every cached share answer (after the clean-up changed lists).
    pub fn forget_shares(&self) {
        self.shares.lock().expect("share entries").clear();
    }

    /// `/internal/v1/access` for one caller, organization, machine and
    /// device, from the cache while fresh.
    pub async fn access(
        &self,
        sub: &str,
        org: &str,
        machine: &str,
        device: Option<&str>,
    ) -> Result<AccessAnswer, String> {
        let key = Key {
            sub: sub.to_owned(),
            org: org.to_owned(),
            machine: machine.to_owned(),
            device: device.unwrap_or_default().to_owned(),
        };
        if let Some(e) = self.entries.lock().expect("org entries").get(&key) {
            if e.fresh(self.config.clock.now()) {
                self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(e.answer.clone());
            }
        }
        let query = {
            let mut q = url::form_urlencoded::Serializer::new(String::new());
            q.append_pair("sub", sub)
                .append_pair("org", org)
                .append_pair("machine", machine);
            if let Some(d) = device {
                q.append_pair("device", d);
            }
            q.finish()
        };
        let r = self
            .get(&format!("/internal/v1/access?{query}"), None)
            .await;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                // The API could not be reached (network, timeout, 5xx): the
                // last "allowed" holds for the stale window; anything else
                // fails closed.
                return match self.stale_allowed(&key, &e) {
                    Some(a) => Ok(a),
                    None => Err(e),
                };
            }
        };
        let answer = if r.status() == reqwest::StatusCode::NOT_FOUND {
            AccessAnswer::default()
        } else if r.status().is_success() {
            r.json::<AccessAnswer>()
                .await
                .map_err(|e| format!("Spaces API access answer: {}", e.without_url()))?
        } else {
            // An unexpected 4xx is not an outage and not a yes: fail closed.
            self.entries.lock().expect("org entries").remove(&key);
            return Err(format!("Spaces API: HTTP {}", r.status()));
        };
        let ttl = answer
            .ttl
            .map(Duration::from_secs)
            .unwrap_or(self.ttl())
            .min(self.ttl());
        let mut entries = self.entries.lock().expect("org entries");
        if entries.len() >= MAX_ENTRIES {
            let now = self.config.clock.now();
            let window = self.stale_window();
            entries.retain(|_, e| now.saturating_duration_since(e.at) < window.max(e.ttl));
            if entries.len() >= MAX_ENTRIES {
                entries.clear();
            }
        }
        entries.insert(
            key,
            Entry {
                answer: answer.clone(),
                at: self.config.clock.now(),
                ttl,
            },
        );
        Ok(answer)
    }

    /// What the organization path says about `sub` (in `orgs`) opening
    /// `machine`, owned by `owner`, from `device`. `device_required`: device
    /// enrollment is on, so the device needs the team's approval.
    ///
    /// A binding alone is not enough: the Spaces API takes any machine id,
    /// so an admin could bind a record someone else owns. An org grants
    /// access to `machine` only while its owner is a member of that same
    /// org (asked with the owner's sub, cached like any answer, fail
    /// closed).
    pub async fn decide(
        &self,
        sub: &str,
        owner: &str,
        orgs: &[OrgClaim],
        machine: &str,
        device: Option<&str>,
        device_required: bool,
    ) -> OrgDecision {
        let mut refused = None;
        let mut unavailable = None;
        for org in orgs {
            match self.access(sub, org.key(), machine, device).await {
                Ok(a) if a.member && a.machine_in_org => {
                    match self.access(owner, org.key(), machine, None).await {
                        Ok(o) if o.member => {}
                        Ok(_) => continue,
                        Err(e) => {
                            unavailable.get_or_insert(e);
                            continue;
                        }
                    }
                    if !device_required || (device.is_some() && a.device_approved) {
                        return OrgDecision::Granted {
                            org: org.key().to_owned(),
                        };
                    }
                    refused.get_or_insert(OrgDecision::DeviceRefused {
                        org: org.key().to_owned(),
                        message: match device {
                            None => "team machines need an enrolled device: run `cua devices enroll` (or sign in again)".into(),
                            Some(d) => format!(
                                "this device ({d}) is waiting for a team admin's approval before it can open {}'s computers. An owner or admin can approve it under Computers at app.cua.ai/machines",
                                org.alias
                            ),
                        },
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    unavailable.get_or_insert(e);
                }
            }
        }
        if let Some(r) = refused {
            return r;
        }
        if let Some(e) = unavailable {
            self.counters.fail_closed.fetch_add(1, Ordering::Relaxed);
            return OrgDecision::Unavailable(e);
        }
        OrgDecision::NotShared
    }

    /// The organizations `sub` reaches `machine` (owned by `owner`)
    /// through, from cached answers only (the synchronous paths: re-checks
    /// after a change and the mesh). An answer the API gave stays until it
    /// is replaced or a membership push drops it. The owner must be a
    /// cached member of the same org: a push removing the owner drops that
    /// answer, so everyone reaching the machine through the org is cut.
    pub fn cached_orgs(&self, sub: &str, owner: &str, machine: &str) -> Vec<String> {
        self.cached_org_roles(sub, owner, machine)
            .into_iter()
            .map(|(org, _)| org)
            .collect()
    }

    /// Like [`Self::cached_orgs`], each with whether the team's role on the
    /// computer is watch only (`machine_role` `viewer`).
    pub fn cached_org_roles(&self, sub: &str, owner: &str, machine: &str) -> Vec<(String, bool)> {
        let entries = self.entries.lock().expect("org entries");
        let owner_member = |org: &str| {
            entries
                .get(&Key {
                    sub: owner.to_owned(),
                    org: org.to_owned(),
                    machine: machine.to_owned(),
                    device: String::new(),
                })
                .is_some_and(|e| e.answer.member)
        };
        let map: std::collections::BTreeMap<String, bool> = entries
            .iter()
            .filter(|(k, e)| {
                k.sub == sub
                    && k.machine == machine
                    && e.answer.member
                    && e.answer.machine_in_org
                    && owner_member(&k.org)
            })
            .map(|(k, e)| (k.org.clone(), e.answer.viewer_only()))
            .collect();
        map.into_iter().collect()
    }

    /// Whether a cached answer approved `device` of `sub` in any of `orgs`.
    pub fn cached_device_approved(&self, sub: &str, orgs: &[String], device: &str) -> bool {
        let entries = self.entries.lock().expect("org entries");
        entries.iter().any(|(k, e)| {
            k.sub == sub && k.device == device && orgs.contains(&k.org) && e.answer.device_approved
        })
    }

    /// The machines of organization `org` (ETag-revalidated once the
    /// cached list is older than the TTL).
    pub async fn org_machines(&self, org: &str) -> Result<Vec<String>, String> {
        let cached = self
            .machines
            .lock()
            .expect("org machines")
            .get(org)
            .cloned();
        if let Some(c) = &cached {
            if self.config.clock.now().saturating_duration_since(c.at) < self.ttl() {
                self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(c.machines.clone());
            }
        }
        let path = format!(
            "/internal/v1/orgs/{}/machines",
            url::form_urlencoded::byte_serialize(org.as_bytes()).collect::<String>()
        );
        let r = match self
            .get(&path, cached.as_ref().and_then(|c| c.etag.as_deref()))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                if let Some(c) = &cached {
                    let age = self.config.clock.now().saturating_duration_since(c.at);
                    if age < self.stale_window() {
                        self.note_stale("org_machines", org, age, &e);
                        return Ok(c.machines.clone());
                    }
                }
                self.machines.lock().expect("org machines").remove(org);
                return Err(e);
            }
        };
        let status = r.status();
        let etag = r
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let machines = if status == reqwest::StatusCode::NOT_MODIFIED {
            cached.map(|c| c.machines).unwrap_or_default()
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Vec::new()
        } else if status.is_success() {
            let v: serde_json::Value = r
                .json()
                .await
                .map_err(|e| format!("Spaces API machine list: {}", e.without_url()))?;
            machine_ids(&v)
        } else {
            return Err(format!("Spaces API: HTTP {status}"));
        };
        self.machines.lock().expect("org machines").insert(
            org.to_owned(),
            MachineList {
                machines: machines.clone(),
                etag,
                at: self.config.clock.now(),
            },
        );
        Ok(machines)
    }

    /// A membership push: drops what is cached for `sub` in `org` (every
    /// member when `sub` is `None`) and that org's machine list. Returns the
    /// machines whose sessions must be re-checked.
    pub fn membership_changed(&self, org: &str, sub: Option<&str>) -> Vec<String> {
        self.counters
            .membership_pushes
            .fetch_add(1, Ordering::Relaxed);
        let mut machines: BTreeSet<String> = BTreeSet::new();
        self.entries.lock().expect("org entries").retain(|k, _| {
            let hit = k.org == org && sub.is_none_or(|s| k.sub == s);
            if hit {
                machines.insert(k.machine.clone());
            }
            !hit
        });
        if let Some(list) = self.machines.lock().expect("org machines").remove(org) {
            machines.extend(list.machines);
        }
        machines.into_iter().collect()
    }

    /// Records one shadow comparison; logs a disagreement.
    pub fn shadow(&self, what: &str, sub: &str, machine: &str, file: bool, org: &OrgDecision) {
        self.counters.shadow_checks.fetch_add(1, Ordering::Relaxed);
        if file == org.granted() {
            return;
        }
        self.counters
            .shadow_mismatches
            .fetch_add(1, Ordering::Relaxed);
        // Every mismatch is counted; a log line at most once a minute per
        // caller, machine and outcome (requests repeat).
        {
            let key = format!("{what}:{sub}:{machine}:{file}:{}", org.label());
            let mut logged = self.logged.lock().expect("shadow log");
            if logged
                .get(&key)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
            {
                return;
            }
            if logged.len() > 10_000 {
                logged.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
            logged.insert(key, Instant::now());
        }
        let detail = match org {
            OrgDecision::Unavailable(e) => e.as_str(),
            OrgDecision::DeviceRefused { message, .. } => message.as_str(),
            _ => "",
        };
        tracing::warn!(
            target: "cua_relay::teams_shadow",
            what,
            sub = %crate::ident::tag(sub),
            machine,
            file = if file { "granted" } else { "refused" },
            org = org.label(),
            detail,
            "teams shadow mismatch"
        );
    }

    /// Prometheus lines.
    pub fn metrics(&self) -> String {
        let c = &self.counters;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        format!(
            "# TYPE cua_relay_teams_source gauge\ncua_relay_teams_source{{source=\"{}\"}} 1\n\
             # TYPE cua_relay_org_api_calls_total counter\ncua_relay_org_api_calls_total {}\n\
             # TYPE cua_relay_org_api_errors_total counter\ncua_relay_org_api_errors_total {}\n\
             # TYPE cua_relay_org_cache_hits_total counter\ncua_relay_org_cache_hits_total {}\n\
             # TYPE cua_relay_org_fail_closed_total counter\ncua_relay_org_fail_closed_total {}\n\
             # TYPE cua_relay_org_stale_served_total counter\ncua_relay_org_stale_served_total {}\n\
             # TYPE cua_relay_teams_shadow_checks_total counter\ncua_relay_teams_shadow_checks_total {}\n\
             # TYPE cua_relay_teams_shadow_mismatches_total counter\ncua_relay_teams_shadow_mismatches_total {}\n\
             # TYPE cua_relay_org_membership_pushes_total counter\ncua_relay_org_membership_pushes_total {}\n\
             # TYPE cua_relay_org_service_token_fetches_total counter\ncua_relay_org_service_token_fetches_total {}\n\
             # TYPE cua_relay_org_service_token_errors_total counter\ncua_relay_org_service_token_errors_total {}\n\
             # TYPE cua_relay_org_service_token_refused_total counter\ncua_relay_org_service_token_refused_total {}\n",
            self.config.source.as_str(),
            get(&c.api_calls),
            get(&c.api_errors),
            get(&c.cache_hits),
            get(&c.fail_closed),
            get(&c.stale_served),
            get(&c.shadow_checks),
            get(&c.shadow_mismatches),
            get(&c.membership_pushes),
            get(&c.token_fetches),
            get(&c.token_errors),
            get(&c.token_refused),
        )
    }
}

/// Machine ids from a machine-list reply: `{"machines": [...]}` or a bare
/// array, of ids or of objects with `machine_id` / `id`.
fn machine_ids(v: &serde_json::Value) -> Vec<String> {
    let list = v
        .get("machines")
        .and_then(|m| m.as_array())
        .or_else(|| v.as_array());
    list.map(|a| {
        a.iter()
            .filter_map(|m| {
                m.as_str()
                    .or_else(|| m.get("machine_id").and_then(|x| x.as_str()))
                    .or_else(|| m.get("id").and_then(|x| x.as_str()))
                    .map(str::to_owned)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// An error with its causes ("error sending request: operation timed out"),
/// so the log says whether a lookup timed out, was refused or was reset.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn parses_sources() {
        assert_eq!(TeamsSource::parse("").unwrap(), TeamsSource::File);
        assert_eq!(TeamsSource::parse("Shadow").unwrap(), TeamsSource::Shadow);
        assert_eq!(TeamsSource::parse("org").unwrap(), TeamsSource::Org);
        assert!(TeamsSource::parse("api").is_err());
    }

    #[test]
    fn reads_machine_lists() {
        let v = serde_json::json!({"machines": [{"machine_id": "a"}, {"id": "b"}, "c"]});
        assert_eq!(machine_ids(&v), vec!["a", "b", "c"]);
        assert!(machine_ids(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn service_tokens_are_replaced_at_half_their_lifetime() {
        assert_eq!(token_refresh_after(Some(300)), Duration::from_secs(150));
        assert_eq!(token_refresh_after(None), Duration::from_secs(30));
        assert_eq!(token_refresh_after(Some(0)), Duration::from_secs(1));
        // The CUA-2959 case: Keycloak 60 s behind the API, 30 s leeway. The
        // token must be replaced before the API (true clock) sees it as
        // expired: at 300 - 60 + 30 = 270 s of real age.
        assert!(token_refresh_after(Some(300)) < Duration::from_secs(270 - 60));
    }

    /// A fake token endpoint minting `t1`, `t2`, ... and a fake Spaces API
    /// answering `/internal/v1/entitlements/x` 200 only to the tokens in
    /// `accepted` (401 otherwise). Returns (base URL, mints, API hits).
    async fn fake_auth_api(
        accepted: &'static [&'static str],
    ) -> (String, Arc<AtomicU64>, Arc<AtomicU64>) {
        let mints = Arc::new(AtomicU64::new(0));
        let hits = Arc::new(AtomicU64::new(0));
        let (m, h) = (mints.clone(), hits.clone());
        let app = axum::Router::new()
            .route(
                "/token",
                axum::routing::post(move || {
                    let n = m.fetch_add(1, Ordering::Relaxed) + 1;
                    async move {
                        axum::Json(serde_json::json!({
                            "access_token": format!("t{n}"), "expires_in": 300
                        }))
                    }
                }),
            )
            .route(
                "/internal/v1/entitlements/x",
                axum::routing::get(move |headers: axum::http::HeaderMap| {
                    h.fetch_add(1, Ordering::Relaxed);
                    async move {
                        let auth = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        if accepted.iter().any(|t| auth == format!("Bearer {t}")) {
                            (axum::http::StatusCode::OK, r#"{"plan":"pro"}"#)
                        } else {
                            (
                                axum::http::StatusCode::UNAUTHORIZED,
                                r#"{"error":"unauthorized"}"#,
                            )
                        }
                    }
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, mints, hits)
    }

    fn authed_org(url: &str) -> OrgAccess {
        OrgAccess::new(OrgAccessConfig {
            api_url: Some(url.to_owned()),
            token_url: Some(format!("{url}/token")),
            client_id: "cua-relay-svc".into(),
            client_secret: Some("test-secret".into()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn a_refused_cached_token_is_replaced_and_not_an_api_error() {
        // The API refuses t1 (it thinks it expired), accepts t2.
        let (url, mints, hits) = fake_auth_api(&["t2"]).await;
        let org = authed_org(&url);
        let r = org.get("/internal/v1/entitlements/x", None).await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        assert_eq!(mints.load(Ordering::Relaxed), 2);
        assert_eq!(hits.load(Ordering::Relaxed), 2);
        assert_eq!(org.counters.api_errors.load(Ordering::Relaxed), 0);
        assert_eq!(org.counters.token_refused.load(Ordering::Relaxed), 1);
        // The new token is cached: the next call mints nothing.
        org.get("/internal/v1/entitlements/x", None).await.unwrap();
        assert_eq!(mints.load(Ordering::Relaxed), 2);
        let m = org.metrics();
        assert!(m.contains("cua_relay_org_api_errors_total 0\n"), "{m}");
        assert!(
            m.contains("cua_relay_org_service_token_refused_total 1\n"),
            "{m}"
        );
        assert!(
            m.contains("cua_relay_org_service_token_fetches_total 2\n"),
            "{m}"
        );
    }

    #[tokio::test]
    async fn a_refused_fresh_token_is_one_api_error_and_final() {
        // A misconfigured client: every token is refused.
        let (url, mints, hits) = fake_auth_api(&[]).await;
        let org = Arc::new(authed_org(&url));
        let err = org
            .get("/internal/v1/entitlements/x", None)
            .await
            .unwrap_err();
        assert!(err.starts_with(TOKEN_REFUSED), "{err}");
        // One retry with a fresh token, never more.
        assert_eq!(mints.load(Ordering::Relaxed), 2);
        assert_eq!(hits.load(Ordering::Relaxed), 2);
        assert_eq!(org.counters.api_errors.load(Ordering::Relaxed), 1);

        // The plan lookup does not ask again on top (no 6x fan-out).
        let plans = crate::plan_source::EntitlementPlans::new(org.clone(), None);
        hits.store(0, Ordering::Relaxed);
        plans.warm("x", Duration::from_secs(5)).await;
        assert_eq!(hits.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn concurrent_refusals_mint_one_new_token() {
        let (url, mints, _) = fake_auth_api(&["t2"]).await;
        let org = Arc::new(authed_org(&url));
        // Cache t1 first.
        let _ = org.service_token().await.unwrap();
        let calls: Vec<_> = (0..4)
            .map(|_| {
                let org = org.clone();
                tokio::spawn(async move { org.get("/internal/v1/entitlements/x", None).await })
            })
            .collect();
        for c in calls {
            assert!(c.await.unwrap().is_ok());
        }
        assert_eq!(mints.load(Ordering::Relaxed), 2);
        assert_eq!(org.counters.api_errors.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn no_api_fails_closed() {
        let org = OrgAccess::new(OrgAccessConfig {
            source: TeamsSource::Org,
            ..Default::default()
        });
        let claims = [OrgClaim {
            alias: "aigis".into(),
            id: Some("o1".into()),
            role: "member",
        }];
        let d = org
            .decide("ada", "owner", &claims, "m1", Some("dev_1"), true)
            .await;
        assert!(matches!(d, OrgDecision::Unavailable(_)), "{d:?}");
        // No organizations in the token: nothing to ask.
        assert_eq!(
            org.decide("ada", "owner", &[], "m1", Some("dev_1"), true)
                .await,
            OrgDecision::NotShared
        );
    }

    /// A fake Spaces API whose `/internal/v1/access` answer is set by tests:
    /// `member:<bool>` or an HTTP status for the whole route.
    async fn fake_api() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Option<u16>>>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        use std::sync::{Arc, Mutex};
        let mode: Arc<Mutex<Option<u16>>> = Arc::default(); // Some(status) = fail with it
        let members: Arc<Mutex<Vec<String>>> = Arc::default();
        let (m2, mem2) = (mode.clone(), members.clone());
        let app = axum::Router::new().route(
            "/internal/v1/access",
            axum::routing::get(
                move |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| {
                    let (m, mem) = (m2.clone(), mem2.clone());
                    async move {
                        if let Some(code) = *m.lock().unwrap() {
                            return (
                                axum::http::StatusCode::from_u16(code).unwrap(),
                                axum::Json(serde_json::json!({})),
                            );
                        }
                        let member = mem.lock().unwrap().contains(&q["sub"]);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "member": member, "machine_in_org": member,
                                "device_approved": member
                            })),
                        )
                    }
                },
            ),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (url, mode, members)
    }

    fn org_for(url: &str, ttl_ms: u64, stale_ms: u64) -> OrgAccess {
        OrgAccess::new(OrgAccessConfig {
            source: TeamsSource::Org,
            api_url: Some(url.to_owned()),
            ttl: Some(Duration::from_millis(ttl_ms)),
            stale_window: Some(Duration::from_millis(stale_ms)),
            ..Default::default()
        })
    }

    fn stale(o: &OrgAccess) -> u64 {
        o.counters.stale_served.load(Ordering::Relaxed)
    }

    #[tokio::test]
    async fn outage_within_the_window_allows() {
        let (url, mode, members) = fake_api().await;
        members.lock().unwrap().push("ada".into());
        let o = org_for(&url, 30, 60_000);
        assert!(o.access("ada", "o1", "m1", None).await.unwrap().member);
        tokio::time::sleep(Duration::from_millis(60)).await; // TTL over
        *mode.lock().unwrap() = Some(503);
        let a = o.access("ada", "o1", "m1", None).await.unwrap();
        assert!(a.member && a.machine_in_org);
        assert_eq!(stale(&o), 1);
        assert!(o.metrics().contains("cua_relay_org_stale_served_total 1"));
    }

    #[tokio::test]
    async fn network_error_within_the_window_allows() {
        let (url, _mode, members) = fake_api().await;
        members.lock().unwrap().push("ada".into());
        let o = org_for(&url, 30, 60_000);
        assert!(o.access("ada", "o1", "m1", None).await.unwrap().member);
        // Same cache, API now unreachable (closed port).
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let o2 = OrgAccess::new(OrgAccessConfig {
            api_url: Some(dead),
            ttl: Some(Duration::from_millis(30)),
            stale_window: Some(Duration::from_secs(60)),
            ..Default::default()
        });
        o2.entries
            .lock()
            .unwrap()
            .extend(o.entries.lock().unwrap().clone());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(o2.access("ada", "o1", "m1", None).await.unwrap().member);
        assert_eq!(stale(&o2), 1);
    }

    #[tokio::test]
    async fn outage_past_the_window_denies() {
        let (url, mode, members) = fake_api().await;
        members.lock().unwrap().push("ada".into());
        let o = org_for(&url, 20, 150);
        assert!(o.access("ada", "o1", "m1", None).await.unwrap().member);
        *mode.lock().unwrap() = Some(503);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(o.access("ada", "o1", "m1", None).await.is_ok());
        tokio::time::sleep(Duration::from_millis(200)).await; // past the window
        assert!(o.access("ada", "o1", "m1", None).await.is_err());
        assert_eq!(stale(&o), 1);
    }

    #[tokio::test]
    async fn explicit_deny_during_the_outage_denies() {
        let (url, mode, members) = fake_api().await;
        members.lock().unwrap().push("ada".into());
        let o = org_for(&url, 20, 60_000);
        assert!(o.access("ada", "o1", "m1", None).await.unwrap().member);
        // Removed from the team: the API answers "not a member" ...
        members.lock().unwrap().clear();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!o.access("ada", "o1", "m1", None).await.unwrap().member);
        // ... and a later outage must not bring the old "yes" back.
        *mode.lock().unwrap() = Some(503);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(o.access("ada", "o1", "m1", None).await.is_err());
        assert_eq!(stale(&o), 0);
        // A 404 ("unknown") is an explicit no as well.
        members.lock().unwrap().push("bo".into());
        *mode.lock().unwrap() = None;
        assert!(o.access("bo", "o1", "m1", None).await.unwrap().member);
        *mode.lock().unwrap() = Some(404);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!o.access("bo", "o1", "m1", None).await.unwrap().member);
    }

    #[tokio::test]
    async fn new_user_during_the_outage_denies() {
        let (url, mode, members) = fake_api().await;
        members.lock().unwrap().push("ada".into());
        let o = org_for(&url, 20, 60_000);
        assert!(o.access("ada", "o1", "m1", None).await.unwrap().member);
        *mode.lock().unwrap() = Some(503);
        assert!(o.access("newbie", "o1", "m1", None).await.is_err());
        // Ada, allowed before but on another machine, is new for that key.
        assert!(o.access("ada", "o1", "m2", None).await.is_err());
        assert_eq!(stale(&o), 0);
    }

    #[allow(dead_code)]
    fn _send_check(o: &'static OrgAccess) {
        fn is_send<T: Send>(_: T) {}
        is_send(o.decide("a", "o", &[], "m", None, true));
        is_send(o.org_machines("o"));
    }
}
