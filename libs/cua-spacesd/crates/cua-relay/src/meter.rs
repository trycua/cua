// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Relay allowance metering (account mode): live remote sessions and relay
//! hours per user, pooled per team, measured against a per-plan allowance.
//!
//! Measure and prompt, plus one refusal: a plan in
//! [`MeterConfig::enforce_plans`] (Pro) cannot open a NEW session past its
//! live-session or hours allowance, on any path a session can take: a
//! WebSocket upgrade, a long gRPC stream (process, terminal and watch
//! RPCs) through the HTTPS proxy ([`Meter::enter_enforced`], 429 or gRPC
//! `ResourceExhausted`), or a mesh session a machine reports
//! ([`Meter::enter_mesh_enforced`]). A machine the user already holds a
//! session on is never refused, live sessions are never cut, and short
//! calls are never refused. Other plans are never refused: past the
//! allowance a client gets an `x-cua-relay-allowance` header on proxied
//! responses and `GET /v1/usage` reports `soft_limit_reached` with an
//! upgrade URL, and the session carries on.
//!
//! - A **live session** is one user on one machine with a stream held open
//!   for at least [`MeterConfig::live_after`] (a desktop view, a terminal,
//!   an agent's process stream, a WebSocket). Short request/response calls
//!   (listing Spaces, health checks, one-off file reads) never start one.
//!   It stays live [`LINGER`] after its last stream closes.
//! - **Relay hours** are the time a user has at least one live session
//!   whose traffic crosses the relay, per calendar month (UTC). Two
//!   sessions at once count once.
//! - **Mesh sessions** (CUA-1506) count the same: the machine's mesh node
//!   reports the streams its peers hold open ([`Meter::enter_mesh`]), and
//!   one user on one machine is one session whichever path its streams
//!   take (HTTPS and the mesh at once count once). A mesh stream crosses
//!   the relay only while its packets are relayed ([`Meter::relayed`]); a
//!   direct mesh session is a live session that uses no relay hours.
//! - A **team** pools its members: their live sessions and hours add up,
//!   against the per-seat allowance times the seats.
//!
//! Cheap by design (CUA-1297): one small in-memory map per user, a 15 s
//! tick, and a tiny JSON file (one line per user) rewritten at most every
//! five minutes and only when something changed.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::State as Extract;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::auth_log::{self, Reason, Route};
use crate::clock::Clock;
use crate::oidc::Identity;
use crate::server::{account_token, bearer, Relay};
use crate::storage::{store, Storage};

/// Response header set on proxied responses once the caller is past its
/// allowance: `exceeded; reasons=hours,sessions; upgrade=<url>`.
pub const ALLOWANCE_HEADER: &str = "x-cua-relay-allowance";
/// How long a session stays live after its last stream closed.
pub const LINGER: Duration = Duration::from_secs(30);
/// How often sessions are swept and hours accrued.
pub const TICK: Duration = Duration::from_secs(15);
/// Least time between two writes of the usage file.
pub const PERSIST_EVERY: Duration = Duration::from_secs(300);
/// Longest `/v1/usage` waits for the plan source to answer.
const PLAN_WARM_TIMEOUT: Duration = Duration::from_secs(3);

/// The plan of anyone the plan source does not know.
pub const DEFAULT_PLAN: &str = "free";
/// Where the upgrade prompt links by default (the pricing page).
pub const DEFAULT_UPGRADE_URL: &str = "https://cua.ai/pricing";
/// Default allowances: Free is 2 live sessions and 20 hours a month. Pro
/// and Teams (per seat) are placeholders until their numbers are set.
pub const DEFAULT_ALLOWANCES: &str = "free=2/20,pro=5/200,teams=5/200";

/// One plan's allowance (per person; per seat for a team).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowance {
    /// Live remote sessions at a time.
    pub live_sessions: u32,
    /// Relay hours per calendar month.
    pub hours: u32,
}

/// Parses `plan=SESSIONS/HOURS,…` (e.g. `free=2/20,pro=5/200`).
pub fn parse_allowances(spec: &str) -> Result<BTreeMap<String, Allowance>, String> {
    let mut plans = BTreeMap::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (plan, rest) = part
            .split_once('=')
            .ok_or_else(|| format!("{part:?}: use plan=SESSIONS/HOURS"))?;
        let (sessions, hours) = rest
            .split_once('/')
            .ok_or_else(|| format!("{part:?}: use plan=SESSIONS/HOURS"))?;
        let parse = |s: &str| {
            s.trim()
                .trim_end_matches('h')
                .parse::<u32>()
                .map_err(|_| format!("{part:?}: not a number: {s:?}"))
        };
        let plan = plan.trim().to_ascii_lowercase();
        if plan.is_empty() {
            return Err(format!("{part:?}: empty plan name"));
        }
        plans.insert(
            plan,
            Allowance {
                live_sessions: parse(sessions)?,
                hours: parse(hours)?,
            },
        );
    }
    if !plans.contains_key(DEFAULT_PLAN) {
        return Err(format!("no allowance for the {DEFAULT_PLAN:?} plan"));
    }
    Ok(plans)
}

/// Metering settings.
#[derive(Debug, Clone)]
pub struct MeterConfig {
    /// Meter at all (off: no tracking, no header, `/v1/usage` says so).
    pub enabled: bool,
    /// Allowance per plan; must include [`DEFAULT_PLAN`].
    pub allowances: BTreeMap<String, Allowance>,
    /// Upgrade link the prompt points at (pricing page or Payment Link).
    pub upgrade_url: String,
    /// Usage file (in memory when unset).
    pub usage_file: Option<PathBuf>,
    /// Plans whose caps (live sessions, relay hours) are enforced: a stream
    /// that would open a NEW session past them is refused (see
    /// [`Meter::enter_enforced`]); live sessions are never cut.
    /// Every other plan is measure and prompt only.
    pub enforce_plans: BTreeSet<String>,
    /// Plans file (`{"users": {...}, "teams": {...}}`), reloaded when it
    /// changes. Unset: everyone is on the default plan.
    pub plans_file: Option<PathBuf>,
    /// How long a stream must stay open to make its session live.
    pub live_after: Duration,
    /// The clock session timing reads (open, live, linger); only tests
    /// advance it.
    pub clock: Clock,
    /// Where usage events go (PostHog batch API), for us; none: logs only.
    pub posthog: Option<PostHog>,
}

/// A PostHog project to send usage events to.
#[derive(Debug, Clone)]
pub struct PostHog {
    /// Batch endpoint, e.g. `https://eu.i.posthog.com/batch/`.
    pub endpoint: String,
    /// Project API key (the public `phc_` capture key).
    pub key: String,
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowances: parse_allowances(DEFAULT_ALLOWANCES).expect("default allowances"),
            upgrade_url: DEFAULT_UPGRADE_URL.into(),
            enforce_plans: parse_plan_set(DEFAULT_ENFORCE_PLANS),
            usage_file: None,
            plans_file: None,
            live_after: Duration::from_secs(10),
            clock: Clock::default(),
            posthog: None,
        }
    }
}

/// Plans refused at their caps by default: none (off). Staging and, once
/// Francesco says so, prod set `CUA_RELAY_ENFORCE_PLANS=free,pro`.
pub const DEFAULT_ENFORCE_PLANS: &str = "";

/// Parses a comma-separated list of plan names.
pub fn parse_plan_set(spec: &str) -> BTreeSet<String> {
    spec.split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Where a user's plan comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    /// The person's own plan (default plan when none).
    pub plan: Option<String>,
    /// The team whose pool they draw from.
    pub team: Option<TeamPlan>,
}

/// A team's plan and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPlan {
    /// Team id.
    pub id: String,
    /// Its plan (an allowance name, per seat).
    pub plan: String,
    /// Paid seats (the pool is this many per-seat allowances; at least 1).
    pub seats: u32,
}

/// Resolves a user's plan and team. The default reads the plans file;
/// another source (an entitlements service) can replace it with
/// [`Meter::set_plan_source`].
pub trait PlanSource: Send + Sync {
    /// The plan and team of `user` (`sub`), with verified `email` if any,
    /// given the relay teams the user belongs to (`memberships`).
    fn assignment(&self, user: &str, email: Option<&str>, memberships: &[Membership])
        -> Assignment;
}

/// A relay team (CUA-1354) a user belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// Team id (`team-<hex>`).
    pub team: String,
    /// Its members now.
    pub members: u32,
}

/// Where the relay's teams come from (the team store).
pub trait TeamDirectory: Send + Sync {
    /// The teams `user` (`sub`) belongs to.
    fn memberships(&self, user: &str) -> Vec<Membership>;
}

/// The plans file: per-user plans and teams by member (sub or verified
/// email, case-insensitive).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlansFile {
    /// `sub` or email to plan.
    #[serde(default)]
    pub users: HashMap<String, String>,
    /// Team id to its plan, seats and members.
    #[serde(default)]
    pub teams: HashMap<String, TeamEntry>,
}

/// A team in the plans file.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamEntry {
    /// Plan (default `teams`).
    #[serde(default = "teams_plan")]
    pub plan: String,
    /// Paid seats (default: the number of members).
    #[serde(default)]
    pub seats: Option<u32>,
    /// Members: `sub` or verified email.
    #[serde(default)]
    pub members: Vec<String>,
}

fn teams_plan() -> String {
    "teams".into()
}

impl PlanSource for PlansFile {
    /// A team pools its members only when the plans file gives it a plan
    /// (a paying team): keyed by its relay team id, or listing its members.
    /// A relay team without an entry leaves each member on their own plan.
    fn assignment(
        &self,
        user: &str,
        email: Option<&str>,
        memberships: &[Membership],
    ) -> Assignment {
        let keys: Vec<String> = std::iter::once(user.to_owned())
            .chain(email.map(str::to_ascii_lowercase))
            .collect();
        let matches = |k: &String| keys.iter().any(|x| x.eq_ignore_ascii_case(k));
        let mut teams: Vec<(&String, &TeamEntry, Option<u32>)> = self
            .teams
            .iter()
            .filter_map(|(id, t)| {
                let relay = memberships.iter().find(|m| &m.team == id);
                (relay.is_some() || t.members.iter().any(matches))
                    .then(|| (id, t, relay.map(|m| m.members)))
            })
            .collect();
        // Deterministic when someone is in two teams.
        teams.sort_by(|a, b| a.0.cmp(b.0));
        let team = teams.first().map(|(id, t, members)| TeamPlan {
            id: (*id).clone(),
            plan: t.plan.to_ascii_lowercase(),
            seats: t
                .seats
                .or(*members)
                .unwrap_or(t.members.len() as u32)
                .max(1),
        });
        let plan = self
            .users
            .iter()
            .find(|(k, _)| matches(k))
            .map(|(_, p)| p.to_ascii_lowercase());
        Assignment { plan, team }
    }
}

/// One user's live session on one machine.
#[derive(Debug)]
struct Session {
    /// Streams open, on any path.
    open: u32,
    /// Of those, streams through the relay's HTTPS proxy.
    relay_open: u32,
    /// Last time traffic of this session crossed the relay (an HTTPS
    /// stream, or relayed mesh packets): hours accrue while it is within
    /// [`LINGER`] (or an HTTPS stream is open).
    relay_seen: Option<Instant>,
    /// When `open` last went from 0 to 1.
    open_since: Option<Instant>,
    live: bool,
    /// Held streams open now (WebSocket upgrades, long gRPC streams, mesh
    /// streams): a held session whether or not the tick
    /// has marked it live yet, so concurrent opens cannot slip past a limit.
    upgrades: u32,
    /// Last time a qualifying (held-open or upgraded) stream was seen:
    /// short requests never extend a live session.
    live_seen: Instant,
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct UserUsage {
    email: Option<String>,
    plan: String,
    team: Option<TeamPlan>,
    secs: u64,
    sessions: HashMap<String, Session>,
    /// Soft-limit reasons already logged this period.
    logged: u8,
    /// `secs` at the last usage report.
    reported: u64,
}

impl UserUsage {
    fn live(&self) -> u32 {
        self.sessions.values().filter(|s| s.live).count() as u32
    }

    /// Sessions that count against an enforced limit: live, or holding an
    /// upgraded stream.
    fn held(&self) -> u32 {
        self.sessions
            .values()
            .filter(|s| s.live || s.upgrades > 0)
            .count() as u32
    }

    /// A live session whose traffic crosses the relay now (relay hours).
    fn on_relay(&self, now: Instant) -> bool {
        self.sessions.values().any(|s| {
            s.live
                && (s.relay_open > 0
                    || s.relay_seen
                        .is_some_and(|t| now.saturating_duration_since(t) < LINGER))
        })
    }
}

#[derive(Default)]
struct State {
    period: String,
    users: HashMap<String, UserUsage>,
    teams: HashMap<String, HashSet<String>>,
    dirty: bool,
    /// Usage events waiting to be sent (bounded by [`MAX_EVENTS`]).
    events: Vec<serde_json::Value>,
}

/// Most usage events held for sending; older ones are dropped.
const MAX_EVENTS: usize = 2000;
/// How often every active user's usage is reported.
pub const REPORT_EVERY: Duration = Duration::from_secs(3600);
/// When the first report goes out after a start.
pub const FIRST_REPORT_AFTER: Duration = Duration::from_secs(300);

/// Saved usage: totals for the current period.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    period: String,
    #[serde(default)]
    users: BTreeMap<String, SavedUser>,
}

#[derive(Serialize, Deserialize)]
struct SavedUser {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    secs: u64,
}

/// The usage file's content for the state as it is.
fn saved_of(st: &State) -> Saved {
    Saved {
        period: st.period.clone(),
        users: st
            .users
            .iter()
            .filter(|(_, u)| u.secs > 0)
            .map(|(k, u)| {
                (
                    k.clone(),
                    SavedUser {
                        email: u.email.clone(),
                        secs: u.secs,
                    },
                )
            })
            .collect(),
    }
}

const OVER_SESSIONS: u8 = 1;
const OVER_HOURS: u8 = 2;

/// Usage against the allowance, as `/v1/usage` and the admin view show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageView {
    /// Metering is on for this relay.
    pub metered: bool,
    /// Calendar month (UTC), `YYYY-MM`.
    pub period: String,
    /// The plan whose allowance applies (the team's when pooled).
    pub plan: String,
    /// The pool, when it is a team's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamView>,
    /// Live remote sessions now (the pool's).
    pub live_sessions: u32,
    /// Allowed live sessions at a time (the pool's).
    pub live_sessions_limit: u32,
    /// Relay hours used this period (the pool's), one decimal.
    pub hours_used: f64,
    /// Relay hours per period (the pool's).
    pub hours_limit: u32,
    /// This user's own hours this period, one decimal.
    pub my_hours_used: f64,
    /// Past an allowance: the client shows the upgrade prompt.
    pub soft_limit_reached: bool,
    /// Which allowances: `sessions`, `hours`.
    pub reasons: Vec<String>,
    /// Where the prompt links.
    pub upgrade_url: String,
}

/// The team part of a [`UsageView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamView {
    /// Team id.
    pub id: String,
    /// Seats in the pool.
    pub seats: u32,
}

/// A row of the admin usage view.
#[derive(Debug, Clone, Serialize)]
pub struct AdminRow {
    /// User id.
    pub user: String,
    /// Verified email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Usage as the user sees it.
    #[serde(flatten)]
    pub usage: UsageView,
}

/// The relay's usage meter.
pub struct Meter {
    config: MeterConfig,
    state: Mutex<State>,
    source: RwLock<Arc<dyn PlanSource>>,
    directory: RwLock<Option<Arc<dyn TeamDirectory>>>,
    plans_mtime: Mutex<Option<SystemTime>>,
    last_persist: Mutex<Instant>,
    /// Where write failures of the usage file are counted and reported.
    storage: Arc<Storage>,
}

/// Why a new session was refused (account over its plan's cap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// `plan_limit_sessions` or `plan_limit_hours`.
    pub code: &'static str,
    /// The plan name (`free`, `pro`, `teams`).
    pub plan: String,
    /// Pooled over a team.
    pub team: bool,
    /// Live sessions held now, or hours used.
    pub used: u32,
    /// The cap.
    pub limit: u32,
    /// Where to upgrade.
    pub upgrade_url: String,
}

impl Refusal {
    /// A plain sentence for the person (and for old clients that print the body).
    pub fn message(&self) -> String {
        let plan = match self.plan.as_str() {
            "pro" => "Pro",
            "free" => "Free",
            "teams" => "Teams",
            other => other,
        };
        let (what, hint) = if self.code == "plan_limit_sessions" {
            ("live sessions", "Close one or upgrade")
        } else {
            ("remote hours this month", "Upgrade to keep going")
        };
        let hint = if self.plan == "pro" && self.code == "plan_limit_sessions" {
            "Close one or upgrade to Teams".to_owned()
        } else {
            hint.to_owned()
        };
        format!(
            "You're using {} of {} {what} on {plan}. {hint}: {}",
            self.used, self.limit, self.upgrade_url
        )
    }
}

/// Which path a metered stream takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The relay's HTTPS machine proxy: always crosses the relay.
    Https,
    /// The mesh, as the machine reported it: crosses the relay only while
    /// its packets are relayed ([`Meter::relayed`]).
    Mesh,
}

/// Held for as long as one proxied stream is open; leaves on drop.
pub struct MeterStream {
    meter: Arc<Meter>,
    user: String,
    machine: String,
    via: Via,
    /// Counted in the session's `upgrades`.
    hold: bool,
}

impl std::fmt::Debug for MeterStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeterStream")
            .field("machine", &self.machine)
            .field("via", &self.via)
            .finish()
    }
}

impl Drop for MeterStream {
    fn drop(&mut self) {
        self.meter
            .leave(&self.user, &self.machine, self.via, self.hold);
    }
}

fn hours(secs: u64) -> f64 {
    (secs as f64 / 360.0).round() / 10.0
}

/// `YYYY-MM` (UTC) of a Unix time.
pub fn period_of(unix_secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}")
}

impl Meter {
    /// A meter, restoring this period's totals from the usage file.
    pub fn open(config: MeterConfig) -> std::io::Result<Self> {
        Self::open_with(config, Arc::default())
    }

    /// [`Meter::open`], reporting usage file writes to `storage`.
    pub fn open_with(config: MeterConfig, storage: Arc<Storage>) -> std::io::Result<Self> {
        let period = period_of(now_secs());
        let mut state = State {
            period: period.clone(),
            ..State::default()
        };
        if let Some(path) = &config.usage_file {
            let (files, bytes) = remove_stale_temps(path)?;
            storage.temps_removed(files as u64, bytes);
            match std::fs::read(path) {
                Ok(raw) => match serde_json::from_slice::<Saved>(&raw) {
                    Ok(saved) if saved.period == period => {
                        for (user, s) in saved.users {
                            state.users.insert(
                                user,
                                UserUsage {
                                    email: s.email,
                                    plan: DEFAULT_PLAN.into(),
                                    secs: s.secs,
                                    ..UserUsage::default()
                                },
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "usage file unreadable; starting empty"),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        let meter = Self {
            config,
            state: Mutex::new(state),
            source: RwLock::new(Arc::new(PlansFile::default())),
            directory: RwLock::new(None),
            plans_mtime: Mutex::new(None),
            last_persist: Mutex::new(Instant::now()),
            storage,
        };
        meter.reload_plans();
        // Restored users get their real plan and team.
        let users: Vec<_> = {
            let st = meter.state.lock().expect("meter");
            st.users
                .iter()
                .map(|(u, x)| (u.clone(), x.email.clone()))
                .collect()
        };
        for (user, email) in users {
            meter.resolve(&user, email.as_deref());
        }
        Ok(meter)
    }

    /// Settings.
    pub fn config(&self) -> &MeterConfig {
        &self.config
    }

    /// Replaces where plans and teams come from (a team directory).
    pub fn set_plan_source(&self, source: Arc<dyn PlanSource>) {
        *self.source.write().expect("plan source") = source;
        let users: Vec<_> = {
            let st = self.state.lock().expect("meter");
            st.users
                .iter()
                .map(|(u, x)| (u.clone(), x.email.clone()))
                .collect()
        };
        for (user, email) in users {
            self.resolve(&user, email.as_deref());
        }
    }

    fn allowance(&self, plan: &str) -> Allowance {
        self.config
            .allowances
            .get(plan)
            .or_else(|| self.config.allowances.get(DEFAULT_PLAN))
            .copied()
            .unwrap_or(Allowance {
                live_sessions: 0,
                hours: 0,
            })
    }

    /// Re-reads the user's plan and team into the state.
    fn resolve(&self, user: &str, email: Option<&str>) {
        let a = self.assignment(user, email);
        let mut st = self.state.lock().expect("meter");
        apply(&mut st, user, email, a);
    }

    fn assignment(&self, user: &str, email: Option<&str>) -> Assignment {
        let memberships = self
            .directory
            .read()
            .expect("team directory")
            .as_ref()
            .map(|d| d.memberships(user))
            .unwrap_or_default();
        self.source
            .read()
            .expect("plan source")
            .assignment(user, email, &memberships)
    }

    /// Re-reads the plan and team of `user` if they are metered now (the
    /// plan source learned something new about them).
    pub fn refresh_user(&self, user: &str) {
        let email = {
            let st = self.state.lock().expect("meter");
            match st.users.get(user) {
                Some(u) => u.email.clone(),
                None => return,
            }
        };
        self.resolve(user, email.as_deref());
    }

    /// Where relay teams come from, so a paying team pools its members.
    pub fn set_team_directory(&self, directory: Arc<dyn TeamDirectory>) {
        *self.directory.write().expect("team directory") = Some(directory);
    }

    /// Records a proxied stream of `who` to `machine` opening. Returns the
    /// guard that records it closing, and whether `who` is past an
    /// allowance (for [`ALLOWANCE_HEADER`]).
    pub fn enter(self: &Arc<Self>, who: &Identity, machine: &str) -> Option<(MeterStream, u8)> {
        self.enter_via(who, machine, Via::Https)
    }

    /// Records a mesh stream of `who` to `machine` opening, as the machine
    /// reported it: the same session as `who`'s HTTPS streams to `machine`.
    pub fn enter_mesh(self: &Arc<Self>, who: &Identity, machine: &str) -> Option<MeterStream> {
        self.enter_via(who, machine, Via::Mesh).map(|(s, _)| s)
    }

    /// Traffic of `user`'s mesh session on `machine` crossed the relay at
    /// `at` (relayed packets): relay hours accrue for it while that is
    /// within [`LINGER`].
    pub fn relayed(&self, user: &str, machine: &str, at: Instant) {
        let mut st = self.state.lock().expect("meter");
        if let Some(s) = st
            .users
            .get_mut(user)
            .and_then(|u| u.sessions.get_mut(machine))
        {
            if s.relay_seen.is_none_or(|t| t < at) {
                s.relay_seen = Some(at);
            }
        }
    }

    /// Like [`Meter::enter`] for a stream that holds a session open (a
    /// WebSocket upgrade or a long gRPC stream), and refuses it when `who`'s plan is enforced and
    /// the stream would open a session past the allowance: `Err` carries the
    /// allowance. Everything is in memory (the plan is resolved from the
    /// plan source's cache, never the network) and one lock is taken.
    pub fn enter_enforced(
        self: &Arc<Self>,
        who: &Identity,
        machine: &str,
    ) -> Result<Option<(MeterStream, u8)>, Refusal> {
        self.enter_inner(who, machine, Via::Https, true)
    }

    /// [`Meter::enter_mesh`] for a stream a peer holds open on a machine,
    /// refused like [`Meter::enter_enforced`] when it would open a new
    /// session past an enforced plan's cap. The caller must not let that
    /// peer's session proceed.
    pub fn enter_mesh_enforced(
        self: &Arc<Self>,
        who: &Identity,
        machine: &str,
    ) -> Result<Option<MeterStream>, Refusal> {
        self.enter_inner(who, machine, Via::Mesh, true)
            .map(|s| s.map(|(s, _)| s))
    }

    fn enter_via(
        self: &Arc<Self>,
        who: &Identity,
        machine: &str,
        via: Via,
    ) -> Option<(MeterStream, u8)> {
        self.enter_inner(who, machine, via, false).ok().flatten()
    }

    fn enter_inner(
        self: &Arc<Self>,
        who: &Identity,
        machine: &str,
        via: Via,
        hold: bool,
    ) -> Result<Option<(MeterStream, u8)>, Refusal> {
        if !self.config.enabled {
            return Ok(None);
        }
        let known = {
            let st = self.state.lock().expect("meter");
            st.users
                .get(&who.user)
                .is_some_and(|u| u.email == who.email && !u.sessions.is_empty())
        };
        // Resolved outside the lock, applied with the stream in one critical
        // section so a sweep cannot drop the user in between.
        let assignment = (!known).then(|| self.assignment(&who.user, who.email.as_deref()));
        let now = self.config.clock.now();
        let over = {
            let mut st = self.state.lock().expect("meter");
            if let Some(a) = assignment {
                apply(&mut st, &who.user, who.email.as_deref(), a);
            }
            if hold {
                let (plan, team, members) = self.pool(&st, &who.user);
                if self.config.enforce_plans.contains(&plan) {
                    let seats = team.map_or(1, |t| t.seats.max(1));
                    let per = self.allowance(&plan);
                    let limit = per.live_sessions.saturating_mul(seats);
                    let hours_limit = per.hours.saturating_mul(seats);
                    let already = st
                        .users
                        .get(&who.user)
                        .and_then(|u| u.sessions.get(machine))
                        .is_some_and(|s| s.live || s.upgrades > 0);
                    let (held, secs) = members
                        .iter()
                        .filter_map(|m| st.users.get(*m))
                        .fold((0u32, 0u64), |(h, t), u| (h + u.held(), t + u.secs));
                    if !already && held >= limit {
                        return Err(Refusal {
                            code: "plan_limit_sessions",
                            plan,
                            team: team.is_some(),
                            used: held,
                            limit,
                            upgrade_url: self.config.upgrade_url.clone(),
                        });
                    }
                    if !already && secs >= u64::from(hours_limit) * 3600 {
                        return Err(Refusal {
                            code: "plan_limit_hours",
                            plan,
                            team: team.is_some(),
                            used: hours(secs) as u32,
                            limit: hours_limit,
                            upgrade_url: self.config.upgrade_url.clone(),
                        });
                    }
                }
            }
            let u = st.users.entry(who.user.clone()).or_default();
            let s = u
                .sessions
                .entry(machine.to_owned())
                .or_insert_with(|| Session {
                    open: 0,
                    relay_open: 0,
                    relay_seen: None,
                    open_since: None,
                    live: false,
                    upgrades: 0,
                    live_seen: now,
                    last_seen: now,
                });
            if s.open == 0 {
                s.open_since = Some(now);
            }
            s.open += 1;
            if hold {
                s.upgrades += 1;
            }
            if via == Via::Https {
                s.relay_open += 1;
                s.relay_seen = Some(now);
            }
            s.last_seen = now;
            self.over(&st, &who.user)
        };
        Ok(Some((
            MeterStream {
                meter: self.clone(),
                user: who.user.clone(),
                machine: machine.to_owned(),
                via,
                hold,
            },
            over,
        )))
    }

    /// Marks a session live right away (an upgraded WebSocket is a session
    /// whatever its length).
    pub fn mark_live(&self, user: &str, machine: &str) {
        let mut st = self.state.lock().expect("meter");
        if let Some(s) = st
            .users
            .get_mut(user)
            .and_then(|u| u.sessions.get_mut(machine))
        {
            s.live = true;
            s.live_seen = self.config.clock.now();
        }
    }

    /// Whether `user` is past an allowance now (see [`allowance_header`]).
    pub fn over_now(&self, user: &str) -> u8 {
        if !self.config.enabled {
            return 0;
        }
        let st = self.state.lock().expect("meter");
        self.over(&st, user)
    }

    fn leave(&self, user: &str, machine: &str, via: Via, hold: bool) {
        let mut st = self.state.lock().expect("meter");
        if let Some(s) = st
            .users
            .get_mut(user)
            .and_then(|u| u.sessions.get_mut(machine))
        {
            let now = self.config.clock.now();
            // A stream that stayed open long enough counts even if it
            // closes between two ticks.
            if s.open_since
                .is_some_and(|t| now.duration_since(t) >= self.config.live_after)
            {
                s.live = true;
                s.live_seen = now;
            }
            s.open = s.open.saturating_sub(1);
            if hold {
                s.upgrades = s.upgrades.saturating_sub(1);
            }
            if via == Via::Https {
                s.relay_open = s.relay_open.saturating_sub(1);
                s.relay_seen = Some(now);
            }
            s.last_seen = now;
            if s.open == 0 {
                s.open_since = None;
            }
        }
    }

    /// The pool `user` draws from: (plan, team, members).
    fn pool<'a>(
        &self,
        st: &'a State,
        user: &'a str,
    ) -> (String, Option<&'a TeamPlan>, Vec<&'a str>) {
        let Some(u) = st.users.get(user) else {
            return (DEFAULT_PLAN.into(), None, vec![user]);
        };
        match &u.team {
            Some(team) => {
                let members = st
                    .teams
                    .get(&team.id)
                    .map(|m| m.iter().map(String::as_str).collect())
                    .unwrap_or_else(|| vec![user]);
                (team.plan.clone(), Some(team), members)
            }
            None => (u.plan.clone(), None, vec![user]),
        }
    }

    fn view_locked(&self, st: &State, user: &str) -> UsageView {
        let (plan, team, members) = self.pool(st, user);
        let per = self.allowance(&plan);
        let seats = team.map_or(1, |t| t.seats.max(1));
        let (mut live, mut secs) = (0u32, 0u64);
        for m in &members {
            if let Some(u) = st.users.get(*m) {
                live += u.live();
                secs += u.secs;
            }
        }
        let live_limit = per.live_sessions.saturating_mul(seats);
        let hours_limit = per.hours.saturating_mul(seats);
        let mut reasons = Vec::new();
        if live > live_limit {
            reasons.push("sessions".to_owned());
        }
        if secs >= u64::from(hours_limit) * 3600 {
            reasons.push("hours".to_owned());
        }
        UsageView {
            metered: self.config.enabled,
            period: st.period.clone(),
            plan,
            team: team.map(|t| TeamView {
                id: t.id.clone(),
                seats,
            }),
            live_sessions: live,
            live_sessions_limit: live_limit,
            hours_used: hours(secs),
            hours_limit,
            my_hours_used: hours(st.users.get(user).map_or(0, |u| u.secs)),
            soft_limit_reached: !reasons.is_empty(),
            reasons,
            upgrade_url: self.config.upgrade_url.clone(),
        }
    }

    fn over(&self, st: &State, user: &str) -> u8 {
        let v = self.view_locked(st, user);
        let mut bits = 0;
        if v.reasons.iter().any(|r| r == "sessions") {
            bits |= OVER_SESSIONS;
        }
        if v.reasons.iter().any(|r| r == "hours") {
            bits |= OVER_HOURS;
        }
        bits
    }

    /// `user`'s usage against its allowance.
    pub fn usage(&self, who: &Identity) -> UsageView {
        if self.config.enabled {
            // Current membership: the verified email may have changed.
            self.resolve(&who.user, who.email.as_deref());
        }
        let st = self.state.lock().expect("meter");
        self.view_locked(&st, &who.user)
    }

    /// Every user metered this period (admin view).
    pub fn all(&self) -> Vec<AdminRow> {
        let st = self.state.lock().expect("meter");
        let mut rows: Vec<_> = st
            .users
            .iter()
            .map(|(user, u)| AdminRow {
                user: user.clone(),
                email: u.email.clone(),
                usage: self.view_locked(&st, user),
            })
            .collect();
        rows.sort_by(|a, b| a.user.cmp(&b.user));
        rows
    }

    /// One tick: sessions open for `live_after` become live, sessions idle
    /// past [`LINGER`] end, users with a live session accrue `elapsed`, and
    /// a new month starts from zero. `now_unix` picks the period.
    pub fn tick(&self, elapsed: Duration, now_unix: u64) {
        if !self.config.enabled {
            return;
        }
        let now = self.config.clock.now();
        let period = period_of(now_unix);
        let mut st = self.state.lock().expect("meter");
        if st.period != period {
            for (user, u) in &st.users {
                if u.secs > 0 {
                    tracing::info!(
                        target: "cua_relay::usage",
                        period = %st.period, user = %crate::ident::tag(user),
                        plan = %u.plan, team = u.team.as_ref().map_or("", |t| t.id.as_str()),
                        hours = hours(u.secs), "relay usage for the month"
                    );
                }
            }
            let closing: Vec<String> = st
                .users
                .iter()
                .filter(|(_, u)| u.secs != u.reported)
                .map(|(k, _)| k.clone())
                .collect();
            for user in closing {
                let event = self.event(&st, &user, "relay_usage");
                push_event(&mut st, event);
            }
            st.period = period;
            for u in st.users.values_mut() {
                u.secs = 0;
                u.logged = 0;
                u.reported = 0;
            }
            st.dirty = true;
        }
        let live_after = self.config.live_after;
        let mut accrued = false;
        for u in st.users.values_mut() {
            u.sessions.retain(|_, s| {
                if s.open > 0
                    && s.open_since
                        .is_some_and(|t| now.duration_since(t) >= live_after)
                {
                    s.live = true;
                    s.live_seen = now;
                }
                if s.live && now.duration_since(s.live_seen) >= LINGER {
                    s.live = false;
                }
                s.live || s.open > 0 || now.duration_since(s.last_seen) < LINGER
            });
            if u.on_relay(now) {
                u.secs += elapsed.as_secs();
                accrued = true;
            }
        }
        st.dirty |= accrued;
        // Log each soft limit once per user and period, for us.
        let users: Vec<String> = st.users.keys().cloned().collect();
        for user in users {
            let over = self.over(&st, &user);
            let u = st.users.get_mut(&user).expect("user");
            let new = over & !u.logged;
            if new != 0 {
                u.logged |= new;
                tracing::info!(
                    target: "cua_relay::usage",
                    user = %crate::ident::tag(&user), plan = %u.plan,
                    team = u.team.as_ref().map_or("", |t| t.id.as_str()),
                    sessions = new & OVER_SESSIONS != 0, hours = new & OVER_HOURS != 0,
                    "relay allowance soft limit reached"
                );
                let event = self.event(&st, &user, "relay_allowance_soft_limit");
                push_event(&mut st, event);
            }
            let u = st.users.get_mut(&user).expect("user");
            // Users with nothing this period and no session are forgotten.
            if u.secs == 0 && u.sessions.is_empty() {
                let team = u.team.as_ref().map(|t| t.id.clone());
                st.users.remove(&user);
                if let Some(team) = team {
                    if let Some(m) = st.teams.get_mut(&team) {
                        m.remove(&user);
                        if m.is_empty() {
                            st.teams.remove(&team);
                        }
                    }
                }
            }
        }
    }

    /// A PostHog event about `user` (never the email; the id is hashed).
    fn event(&self, st: &State, user: &str, name: &str) -> serde_json::Value {
        let v = self.view_locked(st, user);
        serde_json::json!({
            "event": name,
            "distinct_id": posthog_distinct_id(user),
            "properties": {
                "product": "relay",
                "period": v.period,
                "plan": v.plan,
                "team": v.team.as_ref().map(|t| t.id.clone()),
                "seats": v.team.as_ref().map(|t| t.seats),
                "live_sessions": v.live_sessions,
                "live_sessions_limit": v.live_sessions_limit,
                "hours_used": v.my_hours_used,
                "pool_hours_used": v.hours_used,
                "hours_limit": v.hours_limit,
                "soft_limit_reached": v.soft_limit_reached,
                "reasons": v.reasons,
                "$process_person_profile": false,
            },
        })
    }

    /// Queues a `relay_usage` event for every user whose hours moved or who
    /// is live, and returns every queued event.
    pub fn take_events(&self, report: bool) -> Vec<serde_json::Value> {
        let mut st = self.state.lock().expect("meter");
        if report {
            let users: Vec<String> = st
                .users
                .iter()
                .filter(|(_, u)| u.secs != u.reported || u.live() > 0)
                .map(|(k, _)| k.clone())
                .collect();
            for user in users {
                let event = self.event(&st, &user, "relay_usage");
                push_event(&mut st, event);
                let u = st.users.get_mut(&user).expect("user");
                u.reported = u.secs;
            }
        }
        std::mem::take(&mut st.events)
    }

    /// Writes the usage file when something changed (at most every
    /// [`PERSIST_EVERY`] unless `force`): compact JSON streamed to a temp
    /// file that replaces it. A failed write is counted and logged (store
    /// `usage` on `/readyz` and `/metrics`) and the totals stay dirty, so
    /// the next call writes them again.
    pub fn persist(&self, force: bool) {
        let Some(path) = &self.config.usage_file else {
            return;
        };
        {
            let mut last = self.last_persist.lock().expect("persist");
            if !force && last.elapsed() < PERSIST_EVERY {
                return;
            }
            *last = Instant::now();
        }
        let saved = {
            let mut st = self.state.lock().expect("meter");
            if !st.dirty {
                return;
            }
            st.dirty = false;
            saved_of(&st)
        };
        let written = write_private_with(path, |out| {
            serde_json::to_writer(&mut *out, &saved).map_err(std::io::Error::other)
        });
        self.storage.record(store::USAGE, &written);
        if written.is_err() {
            self.state.lock().expect("meter").dirty = true;
        }
    }

    /// Account deletion: forgets `user` (usage, team pool membership, queued
    /// PostHog events about them) and rewrites the usage file without them.
    /// True when the user was known in memory. The file is rewritten every
    /// call, so a retry after a failed write finishes the job.
    pub fn erase_user(&self, user: &str) -> std::io::Result<bool> {
        let saved = {
            let mut st = self.state.lock().expect("meter");
            let mut existed = st.users.remove(user).is_some();
            st.teams.retain(|_, members| {
                existed |= members.remove(user);
                !members.is_empty()
            });
            let id = posthog_distinct_id(user);
            let before = st.events.len();
            st.events
                .retain(|e| e.get("distinct_id").and_then(|d| d.as_str()) != Some(id.as_str()));
            existed |= st.events.len() != before;
            if existed {
                st.dirty = true;
            }
            (existed, saved_of(&st))
        };
        let (existed, saved) = saved;
        if let Some(path) = &self.config.usage_file {
            let written = write_private_with(path, |out| {
                serde_json::to_writer(&mut *out, &saved).map_err(std::io::Error::other)
            });
            self.storage.record(store::USAGE, &written);
            written?;
            // What was just written is current.
            if existed {
                self.state.lock().expect("meter").dirty = false;
            }
        }
        Ok(existed)
    }

    /// Reloads the plans file if it changed.
    pub fn reload_plans(&self) {
        let Some(path) = &self.config.plans_file else {
            return;
        };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        {
            let mut last = self.plans_mtime.lock().expect("plans mtime");
            if *last == mtime {
                return;
            }
            *last = mtime;
        }
        let parsed = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|raw| serde_json::from_slice::<PlansFile>(&raw).map_err(|e| e.to_string()));
        match parsed {
            Ok(plans) => {
                tracing::info!(
                    users = plans.users.len(),
                    teams = plans.teams.len(),
                    "relay plans file loaded"
                );
                self.set_plan_source(Arc::new(plans));
            }
            Err(e) => {
                tracing::warn!(error = %e, "relay plans file unreadable; keeping the last one")
            }
        }
    }

    /// Runs the tick, plans reload and persistence until the process ends.
    pub fn spawn(self: &Arc<Self>) {
        if !self.config.enabled {
            return;
        }
        let meter = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut last = Instant::now();
            // The first report comes FIRST_REPORT_AFTER a start, then hourly.
            let mut next_report = Instant::now() + FIRST_REPORT_AFTER;
            let http = match reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
            {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "no HTTP client: usage events stay in the logs");
                    None
                }
            };
            tracing::info!(
                posthog = meter.config.posthog.is_some() && http.is_some(),
                "relay usage meter running"
            );
            loop {
                interval.tick().await;
                let now = Instant::now();
                meter.tick(now.duration_since(last), now_secs());
                last = now;
                meter.reload_plans();
                meter.persist(false);
                let (Some(ph), Some(http)) = (&meter.config.posthog, &http) else {
                    continue;
                };
                let report = now >= next_report;
                if report {
                    next_report = now + REPORT_EVERY;
                }
                let events = meter.take_events(report);
                if events.is_empty() {
                    continue;
                }
                // Sent off the accounting loop: a slow PostHog never delays
                // the tick. Analytics only, so a failed batch is dropped.
                let (http, ph) = (http.clone(), ph.clone());
                tokio::spawn(async move {
                    for chunk in events.chunks(200) {
                        let body = serde_json::json!({"api_key": ph.key, "batch": chunk});
                        match http.post(&ph.endpoint).json(&body).send().await {
                            Ok(r) if r.status().is_success() => {
                                tracing::info!(events = chunk.len(), "usage events sent")
                            }
                            Ok(r) => {
                                tracing::warn!(status = %r.status(), "usage events not accepted")
                            }
                            Err(e) => tracing::warn!(error = %e, "usage events not sent"),
                        }
                    }
                });
            }
        });
    }
}

/// The PostHog distinct id the relay reports `user` under (a keyed HMAC, see
/// [`crate::ident`]; never the account id or email). Account deletion hands it back so the caller can
/// delete that person in PostHog.
pub fn posthog_distinct_id(user: &str) -> String {
    crate::ident::distinct_id(user)
}

/// Records `user`'s current email, plan and team.
fn apply(st: &mut State, user: &str, email: Option<&str>, a: Assignment) {
    let State { users, teams, .. } = st;
    let u = users.entry(user.to_owned()).or_default();
    u.email = email.map(str::to_owned);
    u.plan = a.plan.unwrap_or_else(|| DEFAULT_PLAN.into());
    if u.team.as_ref().map(|t| &t.id) != a.team.as_ref().map(|t| &t.id) {
        if let Some(old) = &u.team {
            if let Some(members) = teams.get_mut(&old.id) {
                members.remove(user);
                if members.is_empty() {
                    teams.remove(&old.id);
                }
            }
        }
        if let Some(new) = &a.team {
            teams
                .entry(new.id.clone())
                .or_default()
                .insert(user.to_owned());
        }
    }
    u.team = a.team;
}

fn push_event(st: &mut State, event: serde_json::Value) {
    if st.events.len() >= MAX_EVENTS {
        st.events.remove(0);
    }
    st.events.push(event);
}

impl TeamDirectory for crate::teams::Teams {
    fn memberships(&self, user: &str) -> Vec<Membership> {
        crate::teams::Teams::memberships(self, user)
            .into_iter()
            .map(|(team, members)| Membership { team, members })
            .collect()
    }
}

/// The value of [`ALLOWANCE_HEADER`] for `over` (none when within).
pub fn allowance_header(over: u8, upgrade_url: &str) -> Option<HeaderValue> {
    if over == 0 {
        return None;
    }
    let mut reasons = Vec::new();
    if over & OVER_SESSIONS != 0 {
        reasons.push("sessions");
    }
    if over & OVER_HOURS != 0 {
        reasons.push("hours");
    }
    HeaderValue::from_str(&format!(
        "exceeded; reasons={}; upgrade={upgrade_url}",
        reasons.join(",")
    ))
    .ok()
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/usage", get(usage))
        .route("/relay/v1/usage", get(admin_usage))
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": message.into()}))).into_response()
}

/// `GET /v1/usage`: the caller's live sessions and relay hours against its
/// allowance. Needs the account token only (no enrolled device): it shows
/// nothing but the caller's own counts.
async fn usage(Extract(relay): Extract<Relay>, headers: HeaderMap) -> Response {
    let Some(oidc) = &relay.config.oidc else {
        return error(
            StatusCode::NOT_FOUND,
            "account mode is not enabled on this relay",
        );
    };
    if bearer(&headers, "authorization")
        .is_some_and(|t| t.starts_with(crate::directory::MACHINE_TOKEN_PREFIX))
    {
        return error(StatusCode::FORBIDDEN, "machine tokens have no usage");
    }
    let Some(token) = account_token(&headers) else {
        auth_log::rejected(&headers, Route::Machines, Reason::MissingToken, None);
        return error(StatusCode::UNAUTHORIZED, "missing account token");
    };
    match oidc.validate(token).await {
        Ok(who) => {
            // The first read after a purchase or a restart asks the Spaces
            // API before answering, so the app shows the real plan.
            if let Some(plans) = &relay.plans {
                plans.warm(&who.user, PLAN_WARM_TIMEOUT).await;
            }
            Json(relay.meter.usage(&who)).into_response()
        }
        Err(e) => {
            auth_log::rejected(&headers, Route::Machines, e.reason, Some(&e.facts));
            error(
                StatusCode::UNAUTHORIZED,
                format!("invalid account token: {e}"),
            )
        }
    }
}

/// `GET /relay/v1/usage` (admin token): every user metered this period.
async fn admin_usage(Extract(relay): Extract<Relay>, headers: HeaderMap) -> Response {
    if let Err(r) = crate::server::require_admin(&relay, &headers) {
        return r;
    }
    let rows = relay.meter.all();
    Json(serde_json::json!({
        "metered": relay.meter.config().enabled,
        "period": period_of(now_secs()),
        "allowances": relay.meter.config().allowances,
        "upgrade_url": relay.meter.config().upgrade_url,
        "users": rows,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(user: &str, email: &str) -> Identity {
        Identity {
            user: user.into(),
            account: user.into(),
            email: Some(email.into()),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    fn meter(config: MeterConfig) -> Arc<Meter> {
        Arc::new(Meter::open(config).unwrap())
    }

    fn fast() -> MeterConfig {
        MeterConfig {
            live_after: Duration::ZERO,
            ..MeterConfig::default()
        }
    }

    const OCT: u64 = 1_791_000_000; // 2026-10-03
    const NOV: u64 = 1_793_800_000; // 2026-11-04

    #[test]
    fn pro_is_refused_at_the_sixth_session_and_teams_is_not() {
        let m = meter(MeterConfig {
            enforce_plans: parse_plan_set("free,pro"),
            ..fast()
        });
        let mut plans = PlansFile::default();
        plans.users.insert("ada".into(), "pro".into());
        m.set_plan_source(Arc::new(plans));
        let ada = who("ada", "ada@example.com");
        let mut held = Vec::new();
        for i in 0..5 {
            let s = m
                .enter_enforced(&ada, &format!("m{i}"))
                .expect("within the limit");
            held.push(s);
        }
        // A sixth machine is refused with the allowance; no session is left behind.
        assert_eq!(m.enter_enforced(&ada, "m5").unwrap_err().limit, 5);
        assert_eq!(m.enter_enforced(&ada, "m5").unwrap_err().limit, 5);
        // Another stream to a machine already held is fine.
        assert!(m.enter_enforced(&ada, "m0").is_ok());
        // A closed session stays live for LINGER, so a reconnect blip cannot be gamed.
        held.remove(1);
        assert_eq!(m.enter_enforced(&ada, "m5").unwrap_err().limit, 5);
        // The refusal says what and why in a sentence.
        let r = m.enter_enforced(&ada, "m5").unwrap_err();
        assert_eq!(r.code, "plan_limit_sessions");
        assert!(r.message().starts_with(
            "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams"
        ));
        // Free is refused at 2.
        let bob = who("bob", "bob@example.com");
        let _two: Vec<_> = (0..2)
            .map(|i| {
                m.enter_enforced(&bob, &format!("b{i}"))
                    .expect("within free")
            })
            .collect();
        assert_eq!(m.enter_enforced(&bob, "b2").unwrap_err().limit, 2);
        // Off by default: nothing is refused.
        let off = meter(fast());
        let _all: Vec<_> = (0..4)
            .map(|i| {
                off.enter_enforced(&bob, &format!("b{i}"))
                    .expect("flag off")
            })
            .collect();
    }

    #[test]
    fn mesh_sessions_are_refused_like_upgrades_and_count_in_the_same_tally() {
        let m = meter(MeterConfig {
            enforce_plans: parse_plan_set("free,pro"),
            ..fast()
        });
        let mut plans = PlansFile::default();
        plans.users.insert("bob".into(), "free".into());
        m.set_plan_source(Arc::new(plans));
        let bob = who("bob", "bob@example.com");
        // One mesh session and one upgrade fill Free's two.
        let mesh = m.enter_mesh_enforced(&bob, "b0").expect("first");
        let _up = m.enter_enforced(&bob, "b1").expect("second");
        // A third machine is refused on either path, with the same refusal.
        let r = m.enter_mesh_enforced(&bob, "b2").unwrap_err();
        assert_eq!((r.code, r.limit, r.used), ("plan_limit_sessions", 2, 2));
        assert_eq!(m.enter_enforced(&bob, "b2").unwrap_err(), r);
        // A machine already held is never refused, on either path.
        assert!(m.enter_mesh_enforced(&bob, "b1").is_ok());
        assert!(m.enter_enforced(&bob, "b0").is_ok());
        // Closing a mesh session does not cut anything; the linger holds the slot.
        drop(mesh);
        assert!(m.enter_mesh_enforced(&bob, "b2").is_err());
    }

    #[test]
    fn periods_are_utc_months() {
        assert_eq!(period_of(0), "1970-01");
        assert_eq!(period_of(OCT), "2026-10");
        assert_eq!(period_of(NOV), "2026-11");
        assert_eq!(period_of(951_782_400), "2000-02-29".get(..7).unwrap()); // leap day
        assert_eq!(period_of(1_798_761_599), "2026-12"); // 2026-12-31 23:59:59
        assert_eq!(period_of(1_798_761_600), "2027-01");
    }

    #[test]
    fn allowances_parse_and_need_free() {
        let a = parse_allowances("free=2/20, Pro=5/200h").unwrap();
        assert_eq!(
            a["free"],
            Allowance {
                live_sessions: 2,
                hours: 20
            }
        );
        assert_eq!(a["pro"].hours, 200);
        assert!(parse_allowances("pro=5/200").is_err());
        assert!(parse_allowances("free=2").is_err());
        assert!(parse_allowances("free=x/20").is_err());
    }

    /// A plan source whose answer the test changes.
    struct Switch(Mutex<Assignment>);
    impl PlanSource for Switch {
        fn assignment(&self, _: &str, _: Option<&str>, _: &[Membership]) -> Assignment {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn a_plan_change_reaches_open_sessions() {
        // CUA-1659: a user metered as Free (plan not known yet) with three
        // live sessions is past the Free allowance; once the plan source
        // learns they are Pro, refresh_user re-resolves them without a new
        // stream and the notice clears. Pooled Teams seats multiply.
        let m = meter(fast());
        let src = Arc::new(Switch(Mutex::new(Assignment::default())));
        m.set_plan_source(src.clone());
        let ada = who("ada", "ada@example.com");
        let guards: Vec<_> = ["m-1", "m-2", "m-3"]
            .iter()
            .map(|id| {
                let (g, _) = m.enter(&ada, id).unwrap();
                m.mark_live("ada", id);
                g
            })
            .collect();
        assert_eq!(m.over_now("ada"), OVER_SESSIONS);
        *src.0.lock().unwrap() = Assignment {
            plan: Some("pro".into()),
            team: None,
        };
        m.refresh_user("ada");
        assert_eq!(m.over_now("ada"), 0);
        *src.0.lock().unwrap() = Assignment {
            plan: Some("teams".into()),
            team: Some(TeamPlan {
                id: "org-1".into(),
                plan: "teams".into(),
                seats: 3,
            }),
        };
        m.refresh_user("ada");
        let v = m.usage(&ada);
        assert_eq!(
            (v.plan.as_str(), v.live_sessions_limit, v.hours_limit),
            ("teams", 15, 600)
        );
        // Someone not metered is left alone.
        m.refresh_user("nobody");
        drop(guards);
    }

    #[test]
    fn short_requests_are_not_sessions() {
        let m = meter(MeterConfig::default()); // live_after 10 s
        let ada = who("ada", "ada@example.com");
        for _ in 0..5 {
            let (g, over) = m.enter(&ada, "mach-1").unwrap();
            assert_eq!(over, 0);
            drop(g);
        }
        m.tick(TICK, OCT);
        let v = m.usage(&ada);
        assert_eq!(v.live_sessions, 0);
        assert_eq!(v.hours_used, 0.0);
    }

    #[test]
    fn a_held_stream_is_a_session_and_accrues_hours() {
        let m = meter(fast());
        let ada = who("ada", "ada@example.com");
        let (g, _) = m.enter(&ada, "mach-1").unwrap();
        m.tick(Duration::from_secs(3600), OCT);
        let v = m.usage(&ada);
        assert_eq!(v.live_sessions, 1);
        assert_eq!(v.live_sessions_limit, 2);
        assert_eq!(v.plan, "free");
        // Live from this tick on; the next tick accrues.
        m.tick(Duration::from_secs(3600), OCT);
        assert_eq!(m.usage(&ada).hours_used, 2.0);
        drop(g);
        // Lingers, still live within LINGER.
        m.tick(Duration::from_secs(60), OCT);
        assert_eq!(m.usage(&ada).live_sessions, 1);
    }

    #[test]
    fn a_long_stream_closing_between_ticks_counts_and_polling_does_not_extend() {
        let m = meter(MeterConfig {
            live_after: Duration::from_millis(20),
            ..MeterConfig::default()
        });
        let ada = who("ada", "ada@example.com");
        let (g, _) = m.enter(&ada, "mach-1").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        drop(g); // no tick in between
        assert_eq!(m.usage(&ada).live_sessions, 1);
        // Short polls refresh last_seen, never the live session.
        let live_seen = m.state.lock().unwrap().users["ada"].sessions["mach-1"].live_seen;
        drop(m.enter(&ada, "mach-1"));
        let st = m.state.lock().unwrap();
        assert_eq!(st.users["ada"].sessions["mach-1"].live_seen, live_seen);
    }

    #[test]
    fn upgraded_streams_are_live_at_once() {
        let m = meter(MeterConfig::default());
        let ada = who("ada", "ada@example.com");
        let (_g, _) = m.enter(&ada, "mach-1").unwrap();
        m.mark_live("ada", "mach-1");
        assert_eq!(m.usage(&ada).live_sessions, 1);
    }

    #[test]
    fn two_sessions_at_once_count_once_for_hours_and_a_third_is_over() {
        let m = meter(fast());
        let ada = who("ada", "ada@example.com");
        let (_a, _) = m.enter(&ada, "mach-1").unwrap();
        let (_b, _) = m.enter(&ada, "mach-2").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(1800), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (2, 0.5));
        assert!(!v.soft_limit_reached);
        let (_c, _) = m.enter(&ada, "mach-3").unwrap();
        m.tick(Duration::ZERO, OCT);
        let v = m.usage(&ada);
        assert_eq!(v.live_sessions, 3);
        assert_eq!(v.reasons, vec!["sessions"]);
        // The next stream is flagged, and still let through.
        let (_d, over) = m.enter(&ada, "mach-3").unwrap();
        assert_eq!(over, OVER_SESSIONS);
        let h = allowance_header(over, &v.upgrade_url).unwrap();
        assert_eq!(
            h.to_str().unwrap(),
            "exceeded; reasons=sessions; upgrade=https://cua.ai/pricing"
        );
    }

    #[test]
    fn hours_soft_limit_and_monthly_reset() {
        let m = meter(MeterConfig {
            allowances: parse_allowances("free=2/1").unwrap(),
            upgrade_url: "https://buy.example/link".into(),
            ..fast()
        });
        let ada = who("ada", "ada@example.com");
        let (_g, _) = m.enter(&ada, "mach-1").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(3599), OCT);
        assert!(!m.usage(&ada).soft_limit_reached);
        m.tick(Duration::from_secs(1), OCT);
        let v = m.usage(&ada);
        assert_eq!(v.reasons, vec!["hours"]);
        assert_eq!(v.upgrade_url, "https://buy.example/link");
        m.tick(Duration::from_secs(15), NOV);
        let v = m.usage(&ada);
        assert_eq!(v.period, "2026-11");
        assert!(!v.soft_limit_reached);
        assert_eq!(v.hours_used, 0.0);
    }

    #[test]
    fn teams_pool_sessions_and_hours_per_seat() {
        let m = meter(MeterConfig {
            allowances: parse_allowances("free=2/20,teams=1/1").unwrap(),
            ..fast()
        });
        let plans: PlansFile = serde_json::from_value(serde_json::json!({
            "users": {"carol@example.com": "pro"},
            "teams": {"acme": {"seats": 2, "members": ["ada@example.com", "bob"]}}
        }))
        .unwrap();
        m.set_plan_source(Arc::new(plans));
        let ada = who("ada", "ADA@example.com");
        let bob = who("bob", "bob@example.com");
        let (_a, _) = m.enter(&ada, "mach-1").unwrap();
        let (_b, _) = m.enter(&bob, "mach-2").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(3600), OCT);
        let v = m.usage(&ada);
        assert_eq!(v.plan, "teams");
        assert_eq!(
            v.team,
            Some(TeamView {
                id: "acme".into(),
                seats: 2
            })
        );
        // Pool: 2 seats x (1 session, 1 h); 2 live, 2 h used: at the limit.
        assert_eq!((v.live_sessions, v.live_sessions_limit), (2, 2));
        assert_eq!(
            (v.hours_used, v.hours_limit, v.my_hours_used),
            (2.0, 2, 1.0)
        );
        assert_eq!(v.reasons, vec!["hours"]);
        assert_eq!(m.usage(&bob).hours_used, 2.0);
        // Someone outside the team is on their own plan.
        assert_eq!(m.usage(&who("carol", "carol@example.com")).plan, "pro");
        assert_eq!(m.usage(&who("dan", "dan@example.com")).plan, "free");
    }

    #[test]
    fn a_relay_team_pools_only_when_it_has_a_plan() {
        struct Dir;
        impl TeamDirectory for Dir {
            fn memberships(&self, user: &str) -> Vec<Membership> {
                match user {
                    "ada" | "bob" => vec![Membership {
                        team: "team-paid".into(),
                        members: 3,
                    }],
                    "cy" => vec![Membership {
                        team: "team-free".into(),
                        members: 2,
                    }],
                    _ => vec![],
                }
            }
        }
        let m = meter(fast());
        m.set_team_directory(Arc::new(Dir));
        let plans: PlansFile = serde_json::from_value(serde_json::json!({
            "users": {"cy": "pro"},
            "teams": {"team-paid": {"plan": "teams"}}
        }))
        .unwrap();
        m.set_plan_source(Arc::new(plans));
        let v = m.usage(&who("ada", "ada@example.com"));
        assert_eq!(v.plan, "teams");
        assert_eq!(
            v.team,
            Some(TeamView {
                id: "team-paid".into(),
                seats: 3
            })
        );
        assert_eq!((v.live_sessions_limit, v.hours_limit), (15, 600));
        // A team with no plan leaves its members on their own.
        let v = m.usage(&who("cy", "cy@example.com"));
        assert_eq!((v.plan.as_str(), v.team), ("pro", None));
    }

    #[test]
    fn usage_survives_a_restart_and_temps_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machines.usage.json");
        std::fs::write(dir.path().join(".machines.usage.json.dead.tmp"), b"x").unwrap();
        let config = MeterConfig {
            usage_file: Some(path.clone()),
            ..fast()
        };
        let m = meter(config.clone());
        assert!(!dir.path().join(".machines.usage.json.dead.tmp").exists());
        let ada = who("ada", "ada@example.com");
        let (g, _) = m.enter(&ada, "mach-1").unwrap();
        let now = now_secs();
        m.tick(Duration::ZERO, now);
        m.tick(Duration::from_secs(5400), now);
        m.persist(false); // too soon after open: skipped
        assert!(!path.exists());
        m.persist(true);
        drop(g);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.len() < 200, "{raw}");
        let m = meter(config);
        assert_eq!(m.usage(&ada).hours_used, 1.5);
    }

    #[test]
    fn erasing_a_user_drops_usage_events_and_the_file_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machines.usage.json");
        let config = MeterConfig {
            usage_file: Some(path.clone()),
            ..fast()
        };
        let m = meter(config.clone());
        let (ada, bob) = (
            who("ada-sub", "ada@example.com"),
            who("bob-sub", "bob@example.com"),
        );
        let (ga, _) = m.enter(&ada, "mach-1").unwrap();
        let (gb, _) = m.enter(&bob, "mach-2").unwrap();
        let now = now_secs();
        m.tick(Duration::ZERO, now);
        m.tick(Duration::from_secs(3600), now);
        m.persist(true);
        drop((ga, gb));
        assert!(std::fs::read_to_string(&path).unwrap().contains("ada-sub"));
        // Queue the hourly events for both.
        let id = posthog_distinct_id("ada-sub");
        assert!(id.starts_with("relay_") && id.len() == 22);
        {
            let mut st = m.state.lock().unwrap();
            for u in ["ada-sub", "bob-sub"] {
                let e = m.event(&st, u, "relay_usage");
                push_event(&mut st, e);
            }
        }
        assert!(m.erase_user("ada-sub").unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("ada") && raw.contains("bob-sub"), "{raw}");
        let events = m.take_events(false);
        assert_eq!(events.len(), 1);
        assert_ne!(events[0]["distinct_id"], id.as_str());
        assert!(m.all().iter().all(|r| r.user != "ada-sub"));
        // Again: nothing known, same file; and a restart does not bring it back.
        assert!(!m.erase_user("ada-sub").unwrap());
        let m = meter(config);
        assert!(m.all().iter().all(|r| r.user != "ada-sub"));
        assert_eq!(m.usage(&bob).hours_used, 1.0);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_usage_write_is_counted_and_retried() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let storage = Arc::new(Storage::new(Some(state.clone()), Default::default()));
        let path = state.join("machines.usage.json");
        let config = MeterConfig {
            usage_file: Some(path.clone()),
            ..fast()
        };
        let m = Arc::new(Meter::open_with(config.clone(), storage.clone()).unwrap());
        let ada = who("ada", "ada@example.com");
        let (g, _) = m.enter(&ada, "mach-1").unwrap();
        let now = now_secs();
        m.tick(Duration::ZERO, now);
        m.tick(Duration::from_secs(3600), now);
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        m.persist(true);
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        drop(g);
        // Root ignores directory permissions; nothing to check then.
        if storage.failures(store::USAGE) > 0 {
            assert!(!path.exists());
            assert!(!storage.store_ok(store::USAGE));
            assert!(!storage.report().0);
            assert!(m.state.lock().unwrap().dirty, "kept for the next write");
        }
        // The next write saves the totals (nothing else changed since).
        m.persist(true);
        assert!(storage.store_ok(store::USAGE));
        assert_eq!(Meter::open(config).unwrap().usage(&ada).hours_used, 1.0);
    }

    #[test]
    fn usage_events_are_hashed_and_report_movement_only() {
        let m = meter(MeterConfig {
            allowances: parse_allowances("free=2/1").unwrap(),
            ..fast()
        });
        let ada = who("ada", "ada@example.com");
        let (g, _) = m.enter(&ada, "mach-1").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(3600), OCT);
        let events = m.take_events(true);
        let names: Vec<_> = events
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["relay_allowance_soft_limit", "relay_usage"]);
        let raw = serde_json::to_string(&events).unwrap();
        assert!(!raw.contains("ada"), "{raw}");
        assert_eq!(events[1]["properties"]["hours_used"], 1.0);
        assert_eq!(events[1]["properties"]["soft_limit_reached"], true);
        // Between reports only new soft-limit events are sent.
        m.tick(Duration::from_secs(60), OCT);
        assert!(m.take_events(false).is_empty());
        drop(g);
    }

    #[test]
    fn a_direct_mesh_session_is_live_and_uses_no_relay_hours() {
        let m = meter(fast());
        let ada = who("ada", "ada@example.com");
        let g = m.enter_mesh(&ada, "mach-1").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(3600), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (1, 0.0));
        drop(g);
        // Lingers like any session, still without hours.
        m.tick(Duration::from_secs(10), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (1, 0.0));
    }

    #[test]
    fn a_relayed_mesh_session_accrues_hours_while_its_packets_cross_the_relay() {
        let m = meter(fast());
        let ada = who("ada", "ada@example.com");
        let _g = m.enter_mesh(&ada, "mach-1").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.relayed("ada", "mach-1", Instant::now());
        m.tick(Duration::from_secs(1800), OCT);
        assert_eq!(m.usage(&ada).hours_used, 0.5);
        // The path went direct: relayed packets stop, and LINGER after the
        // last one so do the hours (the session stays live).
        let long_ago = Instant::now() - LINGER - Duration::from_secs(1);
        m.state
            .lock()
            .unwrap()
            .users
            .get_mut("ada")
            .unwrap()
            .sessions
            .get_mut("mach-1")
            .unwrap()
            .relay_seen = Some(long_ago);
        m.relayed("ada", "mach-1", long_ago - Duration::from_secs(5)); // older: ignored
        m.tick(Duration::from_secs(1800), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (1, 0.5));
        // Relayed packets for a session that is not open change nothing.
        m.relayed("ada", "mach-9", Instant::now());
        m.relayed("bob", "mach-1", Instant::now());
        assert_eq!(m.all().len(), 1);
    }

    #[test]
    fn https_and_the_mesh_at_once_are_one_session() {
        let m = meter(fast());
        let ada = who("ada", "ada@example.com");
        let (https, _) = m.enter(&ada, "mach-1").unwrap();
        let mesh = m.enter_mesh(&ada, "mach-1").unwrap();
        m.tick(Duration::ZERO, OCT);
        m.tick(Duration::from_secs(3600), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (1, 1.0));
        // The HTTPS stream closes, the mesh one (direct) stays: still one
        // session; hours stop LINGER after the last HTTPS traffic.
        drop(https);
        m.state
            .lock()
            .unwrap()
            .users
            .get_mut("ada")
            .unwrap()
            .sessions
            .get_mut("mach-1")
            .unwrap()
            .relay_seen = Some(Instant::now() - LINGER - Duration::from_secs(1));
        m.tick(Duration::from_secs(3600), OCT);
        let v = m.usage(&ada);
        assert_eq!((v.live_sessions, v.hours_used), (1, 1.0));
        drop(mesh);
    }

    #[test]
    fn mesh_sessions_pool_in_a_team_and_reach_the_soft_limit() {
        let m = meter(MeterConfig {
            allowances: parse_allowances("free=2/20,teams=1/1").unwrap(),
            ..fast()
        });
        let plans: PlansFile = serde_json::from_value(serde_json::json!({
            "teams": {"acme": {"seats": 1, "members": ["ada", "bob"]}}
        }))
        .unwrap();
        m.set_plan_source(Arc::new(plans));
        let ada = who("ada", "ada@example.com");
        let bob = who("bob", "bob@example.com");
        let _a = m.enter_mesh(&ada, "mach-1").unwrap();
        let _b = m.enter_mesh(&bob, "mach-2").unwrap();
        m.tick(Duration::ZERO, OCT);
        let v = m.usage(&bob);
        assert_eq!(
            (v.plan.as_str(), v.live_sessions, v.live_sessions_limit),
            ("teams", 2, 1)
        );
        assert_eq!(v.reasons, vec!["sessions"]);
        assert!(m.usage(&ada).soft_limit_reached);
    }

    #[test]
    fn idle_users_are_forgotten_and_off_meters_nothing() {
        let m = meter(MeterConfig::default());
        let ada = who("ada", "ada@example.com");
        drop(m.enter(&ada, "mach-1"));
        m.tick(Duration::from_secs(15), OCT);
        assert_eq!(m.all().len(), 1); // lingering
        let off = meter(MeterConfig {
            enabled: false,
            ..MeterConfig::default()
        });
        assert!(off.enter(&ada, "mach-1").is_none());
        assert!(!off.usage(&ada).metered);
    }
}
