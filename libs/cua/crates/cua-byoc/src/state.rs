// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `<cua home>/cloud/state.json` (0600): this home's owner id, the
//! connected clouds (names only) and every resource Cua created, with its
//! cloud id. A resource is recorded before it exists in the cloud (by the
//! name Cua chose), so a create that dies half way is still found and
//! cleaned up.
//!
//! **Older cua next to this one.** cua 0.7 and 0.8 read this file too and
//! write it back with only the keys they know, so a link's `auth`,
//! `budget_usd_month`, `max_size` and the closed-month `spent` would be
//! lost after one run of an older cua. Those keys are also kept in
//! `<cua home>/cloud/links.json` ([`Links`]) and put back on the next read
//! when the older cua kept the link as it was. A federated link is written
//! with the profile [`FEDERATED_PROFILE`], which no AWS or Modal config
//! has, so an older cua refuses it instead of using the default profile.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::audit::{AuditLog, CloudAuditEntry};
use crate::model::{Auth, Connection, MaxSize, Resource};

/// The profile a federated (`oidc`, `sealed`) link has on disk. No AWS or
/// Modal config names it, so a cua from before federation that reads the
/// link fails with "profile not found" instead of signing in with the
/// default profile; this cua reads it back as no profile.
pub const FEDERATED_PROFILE: &str = "cua-federated-link";

/// The file's content.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    /// A random id for this cua home, the `cua-owner` tag value.
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    resources: Vec<Resource>,
    /// What machines that are no longer recorded cost this month, per
    /// provider (the rest of [`Store::spent_month`]).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    spent: std::collections::BTreeMap<String, Spent>,
}

/// One provider's closed spend in one month.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Spent {
    /// `YYYY-MM` (UTC).
    month: String,
    /// US dollars.
    usd: f64,
}

/// `<cua home>/cloud/links.json`: what an older cua drops from
/// `state.json` when it writes it back (see the module docs). Deprecated
/// once no supported cua predates federation (CUA-2046).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Links {
    /// Per provider.
    #[serde(default)]
    pub links: std::collections::BTreeMap<String, Link>,
    /// `state.json`'s `spent`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    spent: std::collections::BTreeMap<String, Spent>,
}

/// One link's keys an older cua does not know, plus what identifies the
/// link they belong to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// The link they belong to: the account it reached...
    #[serde(default)]
    pub account: String,
    /// ...and when it was made (an older cua that links again changes it).
    #[serde(default)]
    pub connected_at: String,
    /// The profile it had (empty for a federated link).
    #[serde(default)]
    pub profile: String,
    /// [`Connection::auth`].
    #[serde(default, skip_serializing_if = "Auth::is_cli")]
    pub auth: Auth,
    /// [`Connection::budget_usd_month`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd_month: Option<f64>,
    /// [`Connection::max_size`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<MaxSize>,
}

impl Link {
    /// What `c` has that an older cua would drop, if anything.
    fn of(c: &Connection) -> Option<Link> {
        if c.auth.is_cli() && c.budget_usd_month.is_none() && c.max_size.is_none() {
            return None;
        }
        Some(Link {
            account: c.account.clone(),
            connected_at: c.connected_at.clone(),
            profile: c.profile.clone(),
            auth: c.auth.clone(),
            budget_usd_month: c.budget_usd_month,
            max_size: c.max_size,
        })
    }

    /// Whether `c` is this link after an older cua wrote it back: its new
    /// keys gone, the rest as it was.
    fn stripped(&self, c: &Connection) -> bool {
        c.auth.is_cli()
            && c.budget_usd_month.is_none()
            && c.max_size.is_none()
            && c.account == self.account
            && c.connected_at == self.connected_at
    }
}

/// The account-wide month spend of a linked cloud (OIDC or sealed): this
/// home reports what it spent there, and learns what the account's devices
/// spent together ([`crate::spend`]).
#[async_trait::async_trait]
pub trait AccountSpend: Send + Sync {
    /// Reports `local_usd` (this home's month spend on `conn`) and answers
    /// the account's total, `None` when it is not known (no issuer, an
    /// issuer without spend, offline). `fresh`: ask even when the same
    /// figure was reported a moment ago (a create needs the other devices'
    /// latest; background reports don't).
    async fn sync(&self, conn: &Connection, local_usd: f64, fresh: bool) -> Option<f64>;
    /// The account's total as last heard, without asking.
    fn cached(&self, conn: &Connection) -> Option<f64>;
}

/// How long a create waits for the account's spend before it goes on with
/// what it knows.
const ACCOUNT_SPEND_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// This home's cloud state. Every change is a read-modify-write under a
/// process lock with an atomic rename.
pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
    audit: AuditLog,
    usage: crate::usage::UsageLog,
    account_spend: Mutex<Option<std::sync::Arc<dyn AccountSpend>>>,
}

impl Store {
    /// The store under `home` (`<home>/cloud/state.json`).
    pub fn new(home: &Path) -> Self {
        Store {
            path: home.join("cloud").join("state.json"),
            lock: Mutex::new(()),
            audit: AuditLog::new(home),
            usage: crate::usage::UsageLog::new(home),
            account_spend: Mutex::new(None),
        }
    }

    /// Shares this home's spend on linked clouds with the account through
    /// `s` (set by [`crate::Clouds`] when federation is built in).
    pub fn set_account_spend(&self, s: std::sync::Arc<dyn AccountSpend>) {
        *self.account_spend.lock().expect("account spend") = Some(s);
    }

    fn account_spend_for(&self, conn: &Connection) -> Option<std::sync::Arc<dyn AccountSpend>> {
        if conn.auth.is_cli() {
            return None;
        }
        self.account_spend.lock().expect("account spend").clone()
    }

    /// The month's spend a new machine on `conn` is checked against: this
    /// home's, or the account's when that is higher (a linked cloud's
    /// budget is the account's, whichever device spends it). Asks the
    /// issuer for at most a few seconds; offline, this home's figure (and
    /// the account's last known one) decide.
    pub async fn spent_for_limits(&self, conn: &Connection, now: u64) -> std::io::Result<f64> {
        let local = self.spent_month(&conn.provider, now)?;
        let Some(hook) = self.account_spend_for(conn) else {
            return Ok(local);
        };
        let account =
            match tokio::time::timeout(ACCOUNT_SPEND_WAIT, hook.sync(conn, local, true)).await {
                Ok(v) => v,
                Err(_) => hook.cached(conn),
            };
        Ok(account.map_or(local, |a| a.max(local)))
    }

    /// The month's spend `cloud_status` shows for `conn`: this home's, or
    /// the account's last known total when higher.
    pub fn spent_for_status(&self, conn: &Connection, now: u64) -> f64 {
        let local = self.spent_month(&conn.provider, now).unwrap_or(0.0);
        self.account_spend_for(conn)
            .and_then(|h| h.cached(conn))
            .map_or(local, |a| a.max(local))
    }

    /// Reports this home's spend on `provider` to the account in the
    /// background (a linked cloud only; never blocks or fails the caller).
    pub fn share_spend(&self, provider: &str) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(Some(conn)) = self.connection(provider) else {
            return;
        };
        let Some(hook) = self.account_spend_for(&conn) else {
            return;
        };
        let Ok(local) = self.spent_month(provider, crate::model::now()) else {
            return;
        };
        handle.spawn(async move {
            let _ = tokio::time::timeout(ACCOUNT_SPEND_WAIT, hook.sync(&conn, local, false)).await;
        });
    }

    /// Rotates the audit log at `bytes` instead of 5 MB (tests).
    pub fn with_audit_rotation(mut self, bytes: u64) -> Self {
        self.audit = self.audit.rotating_at(bytes);
        self
    }

    /// Appends `e` to the audit log. The log is a record, not a gate: a
    /// write that fails is logged and never fails the action it describes.
    pub fn audit(&self, e: CloudAuditEntry) {
        if let Err(err) = self.audit.append(e) {
            tracing::warn!(path = %self.audit.path().display(), "cloud audit log not written: {err}");
        }
    }

    /// The newest `limit` audit entries, newest last.
    pub fn audit_entries(&self, limit: usize) -> std::io::Result<Vec<CloudAuditEntry>> {
        self.audit.read(limit)
    }

    /// Every machine run, closed ones from the usage log and the ones
    /// still recorded up to `now` ([`crate::usage`]).
    pub fn runs(&self, now: u64) -> std::io::Result<Vec<crate::usage::Usage>> {
        let mut runs = self.usage.read()?;
        runs.extend(
            self.resources()?
                .iter()
                .filter_map(|r| crate::usage::Usage::of(r, now)),
        );
        Ok(runs)
    }

    /// Where the audit log lives.
    pub fn audit_path(&self) -> &Path {
        self.audit.path()
    }

    /// Where it lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cua home it belongs to.
    pub fn home(&self) -> Option<PathBuf> {
        self.path.parent()?.parent().map(Path::to_path_buf)
    }

    /// `<cua home>/cloud/links.json`.
    pub fn links_path(&self) -> PathBuf {
        self.path.with_file_name("links.json")
    }

    fn read(&self) -> std::io::Result<State> {
        let mut s: State = match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", self.path.display()),
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e),
        };
        self.restore(&mut s);
        Ok(s)
    }

    /// Puts back what an older cua dropped (see the module docs), and
    /// reads a federated link's placeholder profile as no profile.
    fn restore(&self, s: &mut State) {
        let links: Links = std::fs::read(self.links_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for c in &mut s.connections {
            if let Some(l) = links.links.get(&c.provider)
                && l.stripped(c)
            {
                tracing::info!(
                    provider = %c.provider,
                    "restored the link's {} auth and limits an older cua left out",
                    l.auth.as_str()
                );
                c.auth = l.auth.clone();
                c.budget_usd_month = l.budget_usd_month;
                c.max_size = l.max_size;
            }
            if c.profile == FEDERATED_PROFILE {
                c.profile = String::new();
            }
        }
        if s.spent.is_empty() {
            s.spent = links.spent;
        }
    }

    /// Writes `links.json` for `s` (removed when nothing in `s` needs it).
    fn write_links(&self, s: &State) -> std::io::Result<()> {
        let links = Links {
            links: s
                .connections
                .iter()
                .filter_map(|c| Link::of(c).map(|l| (c.provider.clone(), l)))
                .collect(),
            spent: s.spent.clone(),
        };
        let path = self.links_path();
        if links == Links::default() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let mut b = serde_json::to_vec_pretty(&links).expect("links serialize");
        b.push(b'\n');
        write_atomic(&path, &b)
    }

    fn write(&self, s: &State) -> std::io::Result<()> {
        // The sidecar first: a crash between the two leaves links.json
        // ahead of state.json, which a read only applies to a link that
        // matches it.
        self.write_links(s)?;
        let mut on_disk = s.clone();
        for c in &mut on_disk.connections {
            if !c.auth.is_cli() && c.profile.is_empty() {
                c.profile = FEDERATED_PROFILE.into();
            }
        }
        let mut b = serde_json::to_vec_pretty(&on_disk).expect("state serializes");
        b.push(b'\n');
        write_atomic(&self.path, &b)
    }

    fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> std::io::Result<T> {
        let _g = self.lock.lock().expect("cloud state lock");
        let mut s = self.read()?;
        let out = f(&mut s);
        self.write(&s)?;
        Ok(out)
    }

    /// This home's owner id (created on first use): the `cua-owner` tag.
    pub fn owner_id(&self) -> std::io::Result<String> {
        {
            let _g = self.lock.lock().expect("cloud state lock");
            let s = self.read()?;
            if !s.owner_id.is_empty() {
                return Ok(s.owner_id);
            }
        }
        self.update(|s| {
            if s.owner_id.is_empty() {
                s.owner_id = format!("{:016x}", rand::random::<u64>());
            }
            s.owner_id.clone()
        })
    }

    /// The connection for `provider`, if connected.
    pub fn connection(&self, provider: &str) -> std::io::Result<Option<Connection>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self
            .read()?
            .connections
            .into_iter()
            .find(|c| c.provider == provider))
    }

    /// Every connection.
    pub fn connections(&self) -> std::io::Result<Vec<Connection>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self.read()?.connections)
    }

    /// Saves `c` (replacing the provider's earlier connection).
    pub fn connect(&self, c: Connection) -> std::io::Result<()> {
        self.update(|s| {
            s.connections.retain(|x| x.provider != c.provider);
            s.connections.push(c);
        })
    }

    /// Forgets `provider`'s connection; whether there was one.
    pub fn disconnect(&self, provider: &str) -> std::io::Result<bool> {
        self.update(|s| {
            let before = s.connections.len();
            s.connections.retain(|x| x.provider != provider);
            before != s.connections.len()
        })
    }

    /// Every recorded resource.
    pub fn resources(&self) -> std::io::Result<Vec<Resource>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self.read()?.resources)
    }

    /// Records `r` (by provider, type and name, or id): inserts or replaces.
    ///
    /// Who started it ([`crate::model::STARTED_BY`]) is kept from the first
    /// record. A new record gets the agent this process runs for, else
    /// `user`; a record made before cua knew who started machines stays
    /// without one (unknown), so an older home is written back unchanged.
    pub fn record(&self, mut r: Resource) -> std::io::Result<()> {
        let provider = r.provider.clone();
        self.update(|s| {
            let earlier: Vec<&Resource> = s.resources.iter().filter(|x| same(x, &r)).collect();
            let started_by = earlier
                .iter()
                .find_map(|x| x.extra.get(crate::model::STARTED_BY).cloned());
            if !r.extra.contains_key(crate::model::STARTED_BY) {
                let who = match started_by {
                    Some(w) => Some(w),
                    None if earlier.is_empty() => Some(
                        cua_sandbox_core::byoc::caller_agent().unwrap_or_else(|| "user".into()),
                    ),
                    None => None,
                };
                if let Some(who) = who {
                    r.extra.insert(crate::model::STARTED_BY.into(), who);
                }
            }
            s.resources.retain(|x| !same(x, &r));
            s.resources.push(r);
        })?;
        self.share_spend(&provider);
        Ok(())
    }

    /// Forgets `r`. What a machine that was created cost this month is
    /// kept in the month's spend ([`Store::spent_month`]).
    pub fn forget(&self, r: &Resource) -> std::io::Result<()> {
        self.forget_quietly(r)?;
        self.share_spend(&r.provider);
        Ok(())
    }

    fn forget_quietly(&self, r: &Resource) -> std::io::Result<()> {
        self.update(|s| {
            let t = crate::model::now();
            let gone: Vec<Resource> = s.resources.iter().filter(|x| same(x, r)).cloned().collect();
            s.resources.retain(|x| !same(x, r));
            for g in gone.iter().filter(|g| !g.pending()) {
                // Its run, for `cloud_cost` (a record, not a gate).
                if let Some(u) = crate::usage::Usage::of(g, t)
                    && let Err(e) = self.usage.append(&u)
                {
                    tracing::warn!(path = %self.usage.path().display(), "cloud usage not written: {e}");
                }
                let cost = crate::limits::cost_this_month(g, t);
                if cost > 0.0 {
                    let month = crate::limits::month_key(t);
                    let e = s.spent.entry(g.provider.clone()).or_default();
                    if e.month != month {
                        *e = Spent { month, usd: 0.0 };
                    }
                    e.usd += cost;
                }
            }
        })
    }

    /// The estimated US dollars machines Cua created on `provider` cost
    /// this calendar month (UTC) up to `now`: the recorded ones' runtime
    /// at list price plus what the forgotten ones cost.
    pub fn spent_month(&self, provider: &str, now: u64) -> std::io::Result<f64> {
        let _g = self.lock.lock().expect("cloud state lock");
        let s = self.read()?;
        let month = crate::limits::month_key(now);
        let closed = s
            .spent
            .get(provider)
            .filter(|e| e.month == month)
            .map_or(0.0, |e| e.usd);
        let open: f64 = s
            .resources
            .iter()
            .filter(|r| r.provider == provider)
            .map(|r| crate::limits::cost_this_month(r, now))
            .sum();
        Ok(closed + open)
    }
}

/// Writes `bytes` to `path` (0600) through a temporary file and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("state dir");
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The same resource: same provider and type, and the same cloud id (or,
/// while one is pending, the same name).
fn same(a: &Resource, b: &Resource) -> bool {
    a.provider == b.provider
        && a.resource_type == b.resource_type
        && ((!a.id.is_empty() && a.id == b.id) || (!a.name.is_empty() && a.name == b.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_a_reopen_and_pending_ones_resolve_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let owner = s.owner_id().unwrap();
        assert_eq!(owner.len(), 16);
        assert_eq!(Store::new(dir.path()).owner_id().unwrap(), owner);

        let mut r = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-space-1".into(),
            ..Default::default()
        };
        s.record(r.clone()).unwrap();
        assert!(s.resources().unwrap()[0].pending());
        r.id = "i-1".into();
        s.record(r.clone()).unwrap();
        let all = Store::new(dir.path()).resources().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "i-1");
        s.forget(&r).unwrap();
        assert!(s.resources().unwrap().is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(s.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn who_started_a_machine_is_kept_from_its_first_record() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let mut r = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-space-2".into(),
            ..Default::default()
        };
        r.extra
            .insert(crate::model::STARTED_BY.into(), "researcher".into());
        s.record(r.clone()).unwrap();
        // Later updates (its id, its state) never change who started it.
        r.extra.remove(crate::model::STARTED_BY);
        r.id = "i-2".into();
        s.record(r).unwrap();
        let all = s.resources().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].extra[crate::model::STARTED_BY], "researcher");
        assert_eq!(all[0].wire(crate::model::now()).started_by, "researcher");
        // A record with no word at all gets one (the caller, else `user`).
        s.record(Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-space-3".into(),
            ..Default::default()
        })
        .unwrap();
        let fresh = s.resources().unwrap();
        assert!(!fresh[1].extra[crate::model::STARTED_BY].is_empty());
        // A record from before cua knew who started machines stays unknown
        // when it is updated (an older home is written back unchanged).
        let mut old = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-space-4".into(),
            ..Default::default()
        };
        s.update(|st| st.resources.push(old.clone())).unwrap();
        old.id = "i-4".into();
        s.record(old).unwrap();
        let all = s.resources().unwrap();
        let four = all.iter().find(|r| r.name == "cua-space-4").unwrap();
        assert!(!four.extra.contains_key(crate::model::STARTED_BY));
    }

    #[test]
    fn forgotten_machines_keep_counting_toward_the_month() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let t = crate::model::now();
        let mut r = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            id: "i-1".into(),
            created: t.saturating_sub(1800).max(crate::limits::month_start(t)),
            ..Default::default()
        };
        r.extra.insert("usd_per_hour".into(), "2.0".into());
        s.record(r.clone()).unwrap();
        let open = s.spent_month("aws", t).unwrap();
        assert!(open > 0.0 && open <= 1.0 + 1e-6, "{open}");
        s.forget(&r).unwrap();
        let closed = s.spent_month("aws", crate::model::now()).unwrap();
        assert!((closed - open).abs() < 0.01, "{closed} vs {open}");
        assert_eq!(s.spent_month("gcp", t).unwrap(), 0.0);
        // A pending record (never created) costs nothing when forgotten.
        let p = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-aws-x".into(),
            created: t,
            ..Default::default()
        };
        s.record(p.clone()).unwrap();
        s.forget(&p).unwrap();
        assert!((s.spent_month("aws", crate::model::now()).unwrap() - closed).abs() < 0.01);
    }

    #[test]
    fn todays_state_files_read_unchanged() {
        // A state.json as clients before federation wrote it.
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        std::fs::write(s.path(), include_str!("../tests/fixtures/state-v0.json")).unwrap();
        let c = s.connection("aws").unwrap().unwrap();
        assert_eq!(c.auth, crate::model::Auth::Cli);
        assert_eq!(c.budget_usd_month, None);
        assert_eq!(c.max_size, None);
        assert_eq!(c.profile, "cua-byoc-sandbox");
        assert_eq!(c.region, "us-west-2");
        assert_eq!(c.ttl_hours, 8);
        assert_eq!(s.resources().unwrap().len(), 1);
        assert_eq!(s.owner_id().unwrap(), "0123456789abcdef");
        // Written back, a CLI connection looks as it did (no new keys).
        s.connect(c.clone()).unwrap();
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(!text.contains("\"auth\""), "{text}");
        assert!(!text.contains("budget_usd_month"), "{text}");
        assert!(!text.contains("\"spent\""), "{text}");
    }

    #[test]
    fn federated_connections_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let c = Connection {
            provider: "aws".into(),
            region: "us-west-2".into(),
            auth: crate::model::Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces"),
            budget_usd_month: Some(200.0),
            max_size: Some(crate::model::MaxSize {
                vcpu: 4,
                mem_gib: 16,
            }),
            ..Default::default()
        };
        s.connect(c.clone()).unwrap();
        assert_eq!(
            Store::new(dir.path()).connection("aws").unwrap().unwrap(),
            c
        );
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(text.contains("\"kind\": \"oidc\""), "{text}");
    }

    /// What cua 0.7 and 0.8 do to state.json: keep only the keys they know.
    fn as_an_older_cua_writes_it(s: &Store) {
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(s.path()).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        o.retain(|k, _| ["owner_id", "connections", "resources"].contains(&k.as_str()));
        for c in o["connections"].as_array_mut().unwrap() {
            c.as_object_mut().unwrap().retain(|k, _| {
                [
                    "provider",
                    "profile",
                    "region",
                    "zone",
                    "project",
                    "environment",
                    "account",
                    "ttl_hours",
                    "connected_at",
                ]
                .contains(&k.as_str())
            });
        }
        std::fs::write(s.path(), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn oidc_link() -> Connection {
        Connection {
            provider: "aws".into(),
            region: "us-west-2".into(),
            account: "123456789012".into(),
            connected_at: "2026-10-24T10:00:00Z".into(),
            auth: crate::model::Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces"),
            budget_usd_month: Some(200.0),
            max_size: Some(crate::model::MaxSize {
                vcpu: 4,
                mem_gib: 16,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_federated_link_survives_an_older_cua_writing_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let cli = Connection {
            provider: "hetzner".into(),
            region: "nbg1".into(),
            account: "my-project".into(),
            connected_at: "2026-10-20T10:00:00Z".into(),
            budget_usd_month: Some(50.0),
            ..Default::default()
        };
        s.connect(oidc_link()).unwrap();
        s.connect(cli.clone()).unwrap();
        // On disk the federated link names a profile no config has, so an
        // older cua refuses it instead of using the default profile.
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(text.contains(FEDERATED_PROFILE), "{text}");
        assert!(s.links_path().exists());

        as_an_older_cua_writes_it(&s);
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(!text.contains("\"auth\"") && !text.contains("budget_usd_month"));
        let back = Store::new(dir.path());
        assert_eq!(back.connection("aws").unwrap().unwrap(), oidc_link());
        assert_eq!(back.connection("hetzner").unwrap().unwrap(), cli);
        // The next write by this cua puts the keys back in state.json.
        back.record(Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            id: "i-1".into(),
            ..Default::default()
        })
        .unwrap();
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert!(text.contains("\"kind\": \"oidc\"") && text.contains("budget_usd_month"));
    }

    #[test]
    fn a_link_an_older_cua_made_again_is_left_as_it_made_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        s.connect(oidc_link()).unwrap();
        as_an_older_cua_writes_it(&s);
        // The older cua links AWS again with a profile: a new link.
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(s.path()).unwrap()).unwrap();
        v["connections"][0]["profile"] = "work".into();
        v["connections"][0]["connected_at"] = "2026-10-25T09:00:00Z".into();
        std::fs::write(s.path(), serde_json::to_vec(&v).unwrap()).unwrap();
        let c = s.connection("aws").unwrap().unwrap();
        assert_eq!(c.auth, crate::model::Auth::Cli);
        assert_eq!((c.profile.as_str(), c.budget_usd_month), ("work", None));
        // Disconnected by the older cua: nothing comes back, and the
        // sidecar goes with the next write.
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(s.path()).unwrap()).unwrap();
        v["connections"] = serde_json::json!([]);
        std::fs::write(s.path(), serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(s.connections().unwrap().is_empty());
        s.owner_id().unwrap();
        assert!(!s.links_path().exists());
    }

    #[test]
    fn the_months_closed_spend_survives_an_older_cua() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let t = crate::model::now();
        let mut r = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            id: "i-1".into(),
            created: t.saturating_sub(3600).max(crate::limits::month_start(t)),
            ..Default::default()
        };
        r.extra.insert("usd_per_hour".into(), "2.0".into());
        s.record(r.clone()).unwrap();
        s.forget(&r).unwrap();
        let spent = s.spent_month("aws", t).unwrap();
        assert!(spent > 0.0);
        as_an_older_cua_writes_it(&s);
        assert!((Store::new(dir.path()).spent_month("aws", t).unwrap() - spent).abs() < 1e-9);
    }

    #[test]
    fn one_connection_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        for region in ["us-east-1", "us-west-2"] {
            s.connect(Connection {
                provider: "aws".into(),
                region: region.into(),
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(s.connections().unwrap().len(), 1);
        assert_eq!(s.connection("aws").unwrap().unwrap().region, "us-west-2");
        assert!(s.disconnect("aws").unwrap());
        assert!(!s.disconnect("aws").unwrap());
    }
}
