// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The cloud side of public TCP shares (CUA-2602): one firewall rule per
//! share on a Space's own cloud host ([`RuleManager`]), and the share edge
//! ([`EdgeManager`]).
//!
//! Rules of the road, enforced here and tested against [`FakeShareNet`]:
//!
//! - a rule opens exactly one TCP port, one of the computer's published share
//!   slots ([`HOST_SLOTS`]), on a firewall Cua made for that Space; never a
//!   range, never another protocol, never a group Cua does not own;
//! - every rule and edge part carries Cua's tags with an expiry, and is
//!   recorded before it is made;
//! - [`RuleManager::sweep`] closes rules whose share ended or expired, and
//!   any tagged rule no record explains (an orphan of a crash); untagged
//!   rules are never listed, so never touched;
//! - [`EdgeManager::sweep`] removes edges past their expiry, idle ones, and
//!   orphans.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::api::Result;
use crate::model::{Connection, tag};
use crate::share_edge::{EdgeDeployed, EdgePlan, ROLE_EDGE, ROLE_SHARE_RULE, TAG_SHARE};
use cua_sandbox_core::Error;

/// The share slots a Cua cloud machine publishes from its Space (ports on
/// the computer that reach the same port in the Space, where the Space's
/// spacesd forwards each to the shared port while a share holds it).
pub const HOST_SLOTS: (u16, u16) = (26500, 26515);

/// Ports a share may never open, whatever the slot maps to.
pub const REFUSED: &[u16] = &[22, 23, 3389, 5900, 5901];

/// One firewall rule the cloud shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRule {
    /// The cloud's rule id.
    pub id: String,
    /// The firewall it is in.
    pub group: String,
    /// The one port it opens.
    pub port: u16,
    /// Who it lets in.
    pub cidrs: Vec<String>,
    /// Its tags.
    pub tags: BTreeMap<String, String>,
}

/// What a cloud implements for shares.
#[async_trait]
pub trait ShareNetApi: Send + Sync {
    /// The cloud word.
    fn provider(&self) -> &'static str;
    /// Opens TCP `port` of firewall `group` to `cidrs`, tagged `tags`;
    /// returns the rule's id.
    async fn open_port(
        &self,
        conn: &Connection,
        group: &str,
        port: u16,
        cidrs: &[String],
        tags: &BTreeMap<String, String>,
    ) -> Result<String>;
    /// Closes a rule. Closing one already gone is not an error.
    async fn close_port(&self, conn: &Connection, group: &str, rule: &str) -> Result<()>;
    /// Every rule tagged as `owner`'s share rule.
    async fn list_rules(&self, conn: &Connection, owner: &str) -> Result<Vec<OpenRule>>;
    /// Makes the edge of `plan`; nothing is left behind on failure.
    async fn deploy_edge(&self, conn: &Connection, plan: &EdgePlan) -> Result<EdgeDeployed>;
    /// Moves the edge's expiry to `expires` (its tags).
    async fn renew_edge(&self, conn: &Connection, edge: &EdgeDeployed, expires: u64) -> Result<()>;
    /// Removes every part of the edge. Parts already gone are not an error.
    async fn remove_edge(&self, conn: &Connection, edge: &EdgeDeployed) -> Result<()>;
    /// Every edge tagged as `owner`'s.
    async fn list_edges(&self, conn: &Connection, owner: &str) -> Result<Vec<EdgeDeployed>>;
    /// The public address of a computer Cua made (`None`: it has none).
    async fn machine_address(&self, conn: &Connection, instance: &str) -> Result<Option<String>>;
}

/// A Space's own cloud host, as this home recorded it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceMachine {
    /// The cloud word.
    pub provider: String,
    /// The computer.
    pub instance: String,
    /// The Cua firewall it is in.
    pub group: String,
    /// That firewall's tags as recorded.
    pub group_tags: BTreeMap<String, String>,
}

/// The cloud machine that runs `space` (`aws:<name>`), from this home's
/// records; `None` for a Space not in a linked cloud.
pub fn space_machine(resources: &[crate::model::Resource], space: &str) -> Option<SpaceMachine> {
    let inst = resources
        .iter()
        .find(|r| r.resource_type == "instance" && r.sandbox == space && !r.id.is_empty())?;
    let group = inst.extra.get("security_group")?.clone();
    let g = resources
        .iter()
        .find(|r| r.resource_type == "security_group" && r.id == group)?;
    Some(SpaceMachine {
        provider: inst.provider.clone(),
        instance: inst.id.clone(),
        group,
        group_tags: g.tags.clone(),
    })
}

/// The tags of a share rule.
pub fn rule_tags(owner: &str, share: &str, expires: u64) -> BTreeMap<String, String> {
    let mut t = BTreeMap::new();
    t.insert(tag::MANAGED.to_owned(), "true".to_owned());
    t.insert(tag::OWNER.to_owned(), owner.to_owned());
    t.insert(tag::ROLE.to_owned(), ROLE_SHARE_RULE.to_owned());
    t.insert(TAG_SHARE.to_owned(), share.to_owned());
    t.insert(tag::EXPIRES.to_owned(), expires.to_string());
    t
}

fn expires_of(tags: &BTreeMap<String, String>) -> Option<u64> {
    tags.get(tag::EXPIRES).and_then(|v| v.parse().ok())
}

/// One rule this home opened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleRecord {
    /// The share's handle.
    pub share: String,
    /// The connection it was opened on.
    pub conn: Connection,
    /// The firewall (Cua's, of the Space's machine).
    pub group: String,
    /// The slot port opened.
    pub port: u16,
    /// Who may connect.
    pub cidrs: Vec<String>,
    /// The cloud's rule id (empty while the open is under way).
    pub rule: String,
    /// Unix seconds.
    pub expires: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Rules {
    rules: Vec<RuleRecord>,
}

/// What an open asked for.
#[derive(Clone, Debug)]
pub struct OpenRequest<'a> {
    /// The share's handle.
    pub share: &'a str,
    /// The cua home's owner id.
    pub owner: &'a str,
    /// The connection of the Space's cloud.
    pub conn: &'a Connection,
    /// The Space machine's firewall, and its tags as the cloud shows them
    /// (it must be Cua's).
    pub group: &'a str,
    /// The firewall's tags.
    pub group_tags: &'a BTreeMap<String, String>,
    /// The slot port.
    pub port: u16,
    /// Who may connect (empty: anyone).
    pub cidrs: &'a [String],
    /// Unix seconds the share ends.
    pub expires: u64,
}

/// The firewall rules of shares on Spaces' own cloud hosts.
pub struct RuleManager {
    path: PathBuf,
    lock: Mutex<()>,
}

impl RuleManager {
    /// Rules recorded under `<home>/share/cloud-rules.json`.
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join("share").join("cloud-rules.json"),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Rules {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, r: &Rules) -> Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| Error::Cloud(e.to_string()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(r).unwrap_or_default())
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| Error::Cloud(e.to_string()))
    }

    /// The recorded rules.
    pub fn list(&self) -> Vec<RuleRecord> {
        let _g = self.lock.lock().expect("rules");
        self.load().rules
    }

    /// Checks a request against the rules of the road (see the module
    /// docs), before anything is recorded or made.
    pub fn check(req: &OpenRequest<'_>) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidArgument(m));
        if !(HOST_SLOTS.0..=HOST_SLOTS.1).contains(&req.port) || REFUSED.contains(&req.port) {
            return bad(format!(
                "port {} is not one of the computer's share slots ({}-{})",
                req.port, HOST_SLOTS.0, HOST_SLOTS.1
            ));
        }
        if req.group_tags.get(tag::MANAGED).map(String::as_str) != Some("true")
            || req.group_tags.get(tag::OWNER).map(String::as_str) != Some(req.owner)
        {
            return Err(Error::Cloud(
                "that firewall is not one Cua made for this Space; nothing was opened".into(),
            ));
        }
        for c in req.cidrs {
            if crate::share_route::normalize_cidr(c).as_deref() != Some(c.as_str()) {
                return bad(format!("{c:?} is not a CIDR (for example 203.0.113.0/24)"));
            }
        }
        Ok(())
    }

    /// Opens the share's slot: recorded first, then made, then the record
    /// gets the rule's id. A failure removes the record.
    pub async fn open(&self, api: &dyn ShareNetApi, req: &OpenRequest<'_>) -> Result<RuleRecord> {
        Self::check(req)?;
        let cidrs: Vec<String> = if req.cidrs.is_empty() {
            vec!["0.0.0.0/0".into()]
        } else {
            req.cidrs.to_vec()
        };
        let mut rec = RuleRecord {
            share: req.share.to_owned(),
            conn: req.conn.clone(),
            group: req.group.to_owned(),
            port: req.port,
            cidrs: cidrs.clone(),
            rule: String::new(),
            expires: req.expires,
        };
        {
            let _g = self.lock.lock().expect("rules");
            let mut all = self.load();
            if all
                .rules
                .iter()
                .any(|r| r.group == rec.group && r.port == rec.port)
            {
                return Err(Error::InvalidArgument(format!(
                    "share slot {} of this Space is already open",
                    rec.port
                )));
            }
            all.rules.push(rec.clone());
            self.save(&all)?;
        }
        let tags = rule_tags(req.owner, req.share, req.expires);
        match api
            .open_port(req.conn, req.group, req.port, &cidrs, &tags)
            .await
        {
            Ok(id) => {
                rec.rule = id;
                let _g = self.lock.lock().expect("rules");
                let mut all = self.load();
                for r in &mut all.rules {
                    if r.share == rec.share && r.port == rec.port && r.group == rec.group {
                        r.rule = rec.rule.clone();
                    }
                }
                self.save(&all)?;
                Ok(rec)
            }
            Err(e) => {
                self.forget(req.share);
                Err(e)
            }
        }
    }

    fn forget(&self, share: &str) -> Vec<RuleRecord> {
        let _g = self.lock.lock().expect("rules");
        let mut all = self.load();
        let (gone, keep): (Vec<_>, Vec<_>) = all.rules.into_iter().partition(|r| r.share == share);
        all.rules = keep;
        let _ = self.save(&all);
        gone
    }

    /// Closes the rules of `share`.
    pub async fn close(&self, api: &dyn ShareNetApi, share: &str) -> Result<()> {
        let gone = self.forget(share);
        let mut first_err = None;
        for r in gone {
            if r.rule.is_empty() {
                continue;
            }
            if let Err(e) = api.close_port(&r.conn, &r.group, &r.rule).await {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Closes rules whose share is not in `live` or whose expiry passed,
    /// then every rule the cloud tags as `owner`'s that no record explains
    /// (or that expired). Returns the ids closed.
    pub async fn sweep(
        &self,
        api: &dyn ShareNetApi,
        owner: &str,
        conns: &[Connection],
        live: &HashSet<String>,
        now: u64,
    ) -> Vec<String> {
        let mut closed = Vec::new();
        for r in self.list() {
            if !live.contains(&r.share) || r.expires <= now {
                let _ = self.close(api, &r.share).await;
                if !r.rule.is_empty() {
                    closed.push(r.rule.clone());
                }
            }
        }
        let known: HashSet<String> = self.list().into_iter().map(|r| r.rule).collect();
        for conn in conns {
            let Ok(rules) = api.list_rules(conn, owner).await else {
                continue;
            };
            for rule in rules {
                // Only Cua's share rules of this owner are ever listed.
                if rule.tags.get(tag::ROLE).map(String::as_str) != Some(ROLE_SHARE_RULE)
                    || rule.tags.get(tag::OWNER).map(String::as_str) != Some(owner)
                {
                    continue;
                }
                let expired = expires_of(&rule.tags).is_none_or(|e| e <= now);
                let share_live = rule.tags.get(TAG_SHARE).is_some_and(|s| live.contains(s));
                if (!known.contains(&rule.id) || expired || !share_live)
                    && api.close_port(conn, &rule.group, &rule.id).await.is_ok()
                {
                    closed.push(rule.id.clone());
                }
            }
        }
        closed
    }
}

/// The edge this home deployed, and the tunnel token it holds.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EdgeRecord {
    /// The connection it lives on.
    pub conn: Option<Connection>,
    /// It, once made.
    pub edge: Option<EdgeDeployed>,
    /// The tunnel token (only here and on the edge; 0600).
    pub token: String,
    /// Unix seconds a share last used it.
    pub last_used: u64,
}

impl std::fmt::Debug for EdgeRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeRecord")
            .field("conn", &self.conn)
            .field("edge", &self.edge)
            .field("token", &"<redacted>")
            .field("last_used", &self.last_used)
            .finish()
    }
}

/// The share edge of this home (at most one per cloud connection).
pub struct EdgeManager {
    path: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl EdgeManager {
    /// The record under `<home>/share/edge.json`.
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join("share").join("edge.json"),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The record.
    pub fn record(&self) -> EdgeRecord {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, r: &EdgeRecord) -> Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| Error::Cloud(e.to_string()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(r).unwrap_or_default())
            .map_err(|e| Error::Cloud(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| Error::Cloud(e.to_string()))
    }

    /// The edge on `conn`, made from `make_plan(token)` when there is none
    /// (or the recorded one is gone). The token is made once and kept.
    pub async fn ensure(
        &self,
        api: &dyn ShareNetApi,
        conn: &Connection,
        owner: &str,
        now: u64,
        make_plan: impl FnOnce(&str) -> EdgePlan,
    ) -> Result<(EdgeDeployed, String)> {
        let _g = self.lock.lock().await;
        let mut rec = self.record();
        if rec.token.is_empty() {
            rec.token = random_token();
        }
        if let (Some(c), Some(e)) = (&rec.conn, &rec.edge) {
            let live = api
                .list_edges(c, owner)
                .await
                .map(|l| l.iter().any(|x| x.instance == e.instance))
                .unwrap_or(true);
            if c == conn && live {
                let e = e.clone();
                rec.last_used = now;
                self.save(&rec)?;
                return Ok((e, rec.token));
            }
        }
        let plan = make_plan(&rec.token);
        if plan.provider != api.provider() {
            return Err(Error::InvalidArgument(
                "the edge plan is for another cloud".into(),
            ));
        }
        // Recorded before it exists, so a crash mid-deploy is found by the
        // sweep (by its tags) and removed.
        rec.conn = Some(conn.clone());
        rec.edge = None;
        rec.last_used = now;
        self.save(&rec)?;
        let e = api.deploy_edge(conn, &plan).await?;
        rec.edge = Some(e.clone());
        self.save(&rec)?;
        Ok((e, rec.token))
    }

    /// Notes that a share used the edge now, and renews its expiry when it
    /// is within a day of it.
    pub async fn touch(&self, api: &dyn ShareNetApi, now: u64) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut rec = self.record();
        rec.last_used = now;
        if let (Some(c), Some(e)) = (&rec.conn, &mut rec.edge)
            && e.expires <= now + 24 * 3600
        {
            let expires = now + crate::share_edge::EDGE_LEASE_SECS;
            api.renew_edge(c, e, expires).await?;
            e.expires = expires;
        }
        self.save(&rec)
    }

    /// Removes the edge now (Settings, "Remove the share edge", or the
    /// cloud being disconnected).
    pub async fn remove(&self, api: &dyn ShareNetApi) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut rec = self.record();
        if let (Some(c), Some(e)) = (&rec.conn, &rec.edge) {
            api.remove_edge(c, e).await?;
        }
        rec.edge = None;
        rec.conn = None;
        self.save(&rec)
    }

    /// Removes the edge when no share used it for `idle_secs` (and no share
    /// is live on it), when it expired, and every tagged edge on `conns`
    /// that is not the recorded one (an orphan). Returns the instances
    /// removed.
    pub async fn sweep(
        &self,
        api: &dyn ShareNetApi,
        owner: &str,
        conns: &[Connection],
        shares_live: bool,
        now: u64,
        idle_secs: u64,
    ) -> Vec<String> {
        let mut removed = Vec::new();
        let rec = self.record();
        let ours = rec.edge.as_ref().map(|e| e.instance.clone());
        if let (Some(c), Some(e)) = (&rec.conn, &rec.edge) {
            let idle = !shares_live && rec.last_used + idle_secs <= now;
            if (idle || e.expires <= now) && api.remove_edge(c, e).await.is_ok() {
                removed.push(e.instance.clone());
                let mut r = rec.clone();
                r.edge = None;
                r.conn = None;
                let _ = self.save(&r);
            }
        }
        for conn in conns {
            let Ok(edges) = api.list_edges(conn, owner).await else {
                continue;
            };
            for e in edges {
                let orphan = Some(&e.instance) != ours.as_ref() || removed.contains(&e.instance);
                if orphan
                    && !removed.contains(&e.instance)
                    && api.remove_edge(conn, &e).await.is_ok()
                {
                    removed.push(e.instance.clone());
                }
            }
        }
        removed
    }
}

fn random_token() -> String {
    use rand::RngCore as _;
    let mut b = [0u8; 24];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A cloud in memory: rules and edges as the provider would keep them,
/// with a switch to fail the next call (tests, and `cua cloud` dry runs).
#[derive(Default)]
pub struct FakeShareNet {
    /// Open rules.
    pub rules: Mutex<Vec<OpenRule>>,
    /// Deployed edges, with their tags.
    pub edges: Mutex<Vec<(EdgeDeployed, BTreeMap<String, String>)>>,
    /// Fail the next open or deploy with this message.
    pub fail_next: Mutex<Option<String>>,
    /// Calls made, in order (`open 26500`, `close r-1`, `deploy`, ...).
    pub calls: Mutex<Vec<String>>,
    next: std::sync::atomic::AtomicU64,
}

impl FakeShareNet {
    fn id(&self, p: &str) -> String {
        format!(
            "{p}-{}",
            self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
        )
    }
    fn fail(&self) -> Result<()> {
        match self.fail_next.lock().unwrap().take() {
            Some(m) => Err(Error::Cloud(m)),
            None => Ok(()),
        }
    }
    /// A rule made by someone else (untagged) or a crash (tagged, unknown).
    pub fn add_rule(&self, rule: OpenRule) {
        self.rules.lock().unwrap().push(rule);
    }
}

#[async_trait]
impl ShareNetApi for FakeShareNet {
    fn provider(&self) -> &'static str {
        "aws"
    }
    async fn open_port(
        &self,
        _conn: &Connection,
        group: &str,
        port: u16,
        cidrs: &[String],
        tags: &BTreeMap<String, String>,
    ) -> Result<String> {
        self.calls.lock().unwrap().push(format!("open {port}"));
        self.fail()?;
        let id = self.id("sgr");
        self.rules.lock().unwrap().push(OpenRule {
            id: id.clone(),
            group: group.to_owned(),
            port,
            cidrs: cidrs.to_vec(),
            tags: tags.clone(),
        });
        Ok(id)
    }
    async fn close_port(&self, _conn: &Connection, _group: &str, rule: &str) -> Result<()> {
        self.calls.lock().unwrap().push(format!("close {rule}"));
        self.rules.lock().unwrap().retain(|r| r.id != rule);
        Ok(())
    }
    async fn list_rules(&self, _conn: &Connection, owner: &str) -> Result<Vec<OpenRule>> {
        Ok(self
            .rules
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                r.tags.get(tag::MANAGED).map(String::as_str) == Some("true")
                    && r.tags.get(tag::OWNER).map(String::as_str) == Some(owner)
            })
            .cloned()
            .collect())
    }
    async fn deploy_edge(&self, _conn: &Connection, plan: &EdgePlan) -> Result<EdgeDeployed> {
        self.calls.lock().unwrap().push("deploy".into());
        self.fail()?;
        let e = EdgeDeployed {
            provider: plan.provider.clone(),
            region: plan.region.clone(),
            instance: self.id("i"),
            group: self.id("sg"),
            address_id: self.id("eipalloc"),
            ip: "198.51.100.10".into(),
            host: "ec2-198-51-100-10.us-west-2.compute.amazonaws.com".into(),
            ports: plan.ports,
            expires: plan.expires,
        };
        self.edges
            .lock()
            .unwrap()
            .push((e.clone(), plan.tags.clone()));
        Ok(e)
    }
    async fn renew_edge(
        &self,
        _conn: &Connection,
        edge: &EdgeDeployed,
        expires: u64,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("renew {}", edge.instance));
        for (e, t) in self.edges.lock().unwrap().iter_mut() {
            if e.instance == edge.instance {
                e.expires = expires;
                t.insert(tag::EXPIRES.into(), expires.to_string());
            }
        }
        Ok(())
    }
    async fn remove_edge(&self, _conn: &Connection, edge: &EdgeDeployed) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("remove {}", edge.instance));
        self.edges
            .lock()
            .unwrap()
            .retain(|(e, _)| e.instance != edge.instance);
        Ok(())
    }
    async fn machine_address(&self, _conn: &Connection, instance: &str) -> Result<Option<String>> {
        Ok(Some(format!("203.0.113.{}", instance.len() % 250)))
    }
    async fn list_edges(&self, _conn: &Connection, owner: &str) -> Result<Vec<EdgeDeployed>> {
        Ok(self
            .edges
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| {
                t.get(tag::OWNER).map(String::as_str) == Some(owner)
                    && t.get(tag::ROLE).map(String::as_str) == Some(ROLE_EDGE)
            })
            .map(|(e, _)| e.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share_edge::{EdgeInputs, plan};

    const OWNER: &str = "home-1";

    fn conn() -> Connection {
        Connection {
            provider: "aws".into(),
            region: "us-west-2".into(),
            ..Default::default()
        }
    }

    fn ours() -> BTreeMap<String, String> {
        let mut t = BTreeMap::new();
        t.insert(tag::MANAGED.to_owned(), "true".to_owned());
        t.insert(tag::OWNER.to_owned(), OWNER.to_owned());
        t
    }

    fn req<'a>(
        share: &'a str,
        conn: &'a Connection,
        tags: &'a BTreeMap<String, String>,
        port: u16,
        cidrs: &'a [String],
        expires: u64,
    ) -> OpenRequest<'a> {
        OpenRequest {
            share,
            owner: OWNER,
            conn,
            group: "sg-space",
            group_tags: tags,
            port,
            cidrs,
            expires,
        }
    }

    #[tokio::test]
    async fn a_rule_opens_one_slot_tagged_and_closes() {
        let home = tempfile::tempdir().unwrap();
        let m = RuleManager::new(home.path());
        let net = FakeShareNet::default();
        let (c, t) = (conn(), ours());
        let rec = m
            .open(&net, &req("share-1", &c, &t, 26500, &[], 100))
            .await
            .unwrap();
        assert_eq!(rec.cidrs, vec!["0.0.0.0/0"]);
        let rules = net.rules.lock().unwrap().clone();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].port, 26500);
        assert_eq!(rules[0].tags[TAG_SHARE], "share-1");
        assert_eq!(rules[0].tags[tag::ROLE], ROLE_SHARE_RULE);
        assert_eq!(rules[0].tags[tag::EXPIRES], "100");
        assert_eq!(m.list()[0].rule, rules[0].id);
        m.close(&net, "share-1").await.unwrap();
        assert!(net.rules.lock().unwrap().is_empty());
        assert!(m.list().is_empty());
    }

    #[tokio::test]
    async fn nothing_outside_the_share_can_be_opened() {
        let home = tempfile::tempdir().unwrap();
        let m = RuleManager::new(home.path());
        let net = FakeShareNet::default();
        let c = conn();
        let t = ours();
        // Not a share slot: SSH, RDP, a database, the game port itself.
        for port in [22, 3389, 5432, 25565, 26499, 26516, 0] {
            assert!(
                m.open(&net, &req("s", &c, &t, port, &[], 9)).await.is_err(),
                "{port}"
            );
        }
        // A firewall Cua did not make (or another owner's).
        let theirs = BTreeMap::new();
        assert!(
            m.open(&net, &req("s", &c, &theirs, 26500, &[], 9))
                .await
                .is_err()
        );
        let mut other = ours();
        other.insert(tag::OWNER.into(), "someone-else".into());
        assert!(
            m.open(&net, &req("s", &c, &other, 26500, &[], 9))
                .await
                .is_err()
        );
        // Not a CIDR.
        let bad = vec!["10.0.0.0/8 or anything".to_string()];
        assert!(
            m.open(&net, &req("s", &c, &t, 26500, &bad, 9))
                .await
                .is_err()
        );
        assert!(
            net.calls.lock().unwrap().is_empty(),
            "the cloud was never asked"
        );
        assert!(m.list().is_empty());
    }

    #[tokio::test]
    async fn an_allowlist_becomes_the_rules_sources() {
        let home = tempfile::tempdir().unwrap();
        let m = RuleManager::new(home.path());
        let net = FakeShareNet::default();
        let (c, t) = (conn(), ours());
        let allow = vec!["203.0.113.0/24".to_string(), "198.51.100.7/32".to_string()];
        m.open(&net, &req("s", &c, &t, 26501, &allow, 9))
            .await
            .unwrap();
        assert_eq!(net.rules.lock().unwrap()[0].cidrs, allow);
    }

    #[tokio::test]
    async fn one_slot_holds_one_share_and_a_failed_open_leaves_nothing() {
        let home = tempfile::tempdir().unwrap();
        let m = RuleManager::new(home.path());
        let net = FakeShareNet::default();
        let (c, t) = (conn(), ours());
        m.open(&net, &req("a", &c, &t, 26502, &[], 9))
            .await
            .unwrap();
        assert!(
            m.open(&net, &req("b", &c, &t, 26502, &[], 9))
                .await
                .is_err()
        );
        *net.fail_next.lock().unwrap() = Some("throttled".into());
        assert!(
            m.open(&net, &req("c", &c, &t, 26503, &[], 9))
                .await
                .is_err()
        );
        assert_eq!(m.list().len(), 1, "the failed open's record is gone");
    }

    #[tokio::test]
    async fn the_sweep_closes_ended_expired_and_orphaned_rules_only() {
        let home = tempfile::tempdir().unwrap();
        let m = RuleManager::new(home.path());
        let net = FakeShareNet::default();
        let (c, t) = (conn(), ours());
        m.open(&net, &req("live", &c, &t, 26500, &[], 1_000))
            .await
            .unwrap();
        m.open(&net, &req("ended", &c, &t, 26501, &[], 1_000))
            .await
            .unwrap();
        m.open(&net, &req("expired", &c, &t, 26502, &[], 50))
            .await
            .unwrap();
        // A crash between the cloud's open and the record: tagged, unknown.
        net.add_rule(OpenRule {
            id: "sgr-crash".into(),
            group: "sg-space".into(),
            port: 26503,
            cidrs: vec!["0.0.0.0/0".into()],
            tags: rule_tags(OWNER, "live", 1_000),
        });
        // Someone else's rule (no Cua tags): never touched.
        net.add_rule(OpenRule {
            id: "sgr-theirs".into(),
            group: "sg-theirs".into(),
            port: 443,
            cidrs: vec!["0.0.0.0/0".into()],
            tags: BTreeMap::new(),
        });
        let live: HashSet<String> = ["live".to_string(), "expired".to_string()].into();
        let closed = m
            .sweep(&net, OWNER, std::slice::from_ref(&c), &live, 100)
            .await;
        let left: Vec<String> = net
            .rules
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left.contains(&"sgr-theirs".to_string()));
        assert!(closed.contains(&"sgr-crash".to_string()));
        assert_eq!(m.list().len(), 1);
        assert_eq!(m.list()[0].share, "live");
        // A second pass changes nothing.
        assert!(m.sweep(&net, OWNER, &[c], &live, 100).await.is_empty());
    }

    fn edge_plan(token: &str) -> EdgePlan {
        plan(&EdgeInputs {
            provider: "aws",
            region: "us-west-2",
            owner: OWNER,
            token,
            version: "1.0.0",
            now: 10,
        })
    }

    #[test]
    fn a_spaces_machine_is_found_with_its_cua_firewall_only() {
        use crate::model::Resource;
        let group = Resource {
            provider: "aws".into(),
            id: "sg-1".into(),
            resource_type: "security_group".into(),
            tags: ours(),
            ..Default::default()
        };
        let mut inst = Resource {
            provider: "aws".into(),
            id: "i-1".into(),
            resource_type: "instance".into(),
            sandbox: "aws:mc".into(),
            ..Default::default()
        };
        inst.extra.insert("security_group".into(), "sg-1".into());
        let all = vec![group, inst];
        let m = space_machine(&all, "aws:mc").unwrap();
        assert_eq!((m.instance.as_str(), m.group.as_str()), ("i-1", "sg-1"));
        assert_eq!(m.group_tags, ours());
        assert!(space_machine(&all, "aws:other").is_none());
        assert!(
            space_machine(&all[1..], "aws:mc").is_none(),
            "no recorded group: no machine"
        );
    }

    #[tokio::test]
    async fn one_edge_is_made_once_and_reused() {
        let home = tempfile::tempdir().unwrap();
        let em = EdgeManager::new(home.path());
        let net = FakeShareNet::default();
        let c = conn();
        let (a, token) = em.ensure(&net, &c, OWNER, 10, edge_plan).await.unwrap();
        let (b, again) = em.ensure(&net, &c, OWNER, 20, edge_plan).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(token, again);
        assert_eq!(token.len(), 48);
        assert_eq!(net.edges.lock().unwrap().len(), 1);
        // The record is private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(home.path().join("share/edge.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[tokio::test]
    async fn a_gone_edge_is_made_again_and_a_failed_deploy_leaves_no_edge() {
        let home = tempfile::tempdir().unwrap();
        let em = EdgeManager::new(home.path());
        let net = FakeShareNet::default();
        let c = conn();
        let (a, _) = em.ensure(&net, &c, OWNER, 10, edge_plan).await.unwrap();
        net.edges.lock().unwrap().clear();
        let (b, _) = em.ensure(&net, &c, OWNER, 20, edge_plan).await.unwrap();
        assert_ne!(a.instance, b.instance);
        em.remove(&net).await.unwrap();
        *net.fail_next.lock().unwrap() = Some("quota".into());
        assert!(em.ensure(&net, &c, OWNER, 30, edge_plan).await.is_err());
        assert!(em.record().edge.is_none());
        assert!(net.edges.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_edge_sweep_removes_idle_expired_and_orphaned_edges() {
        let home = tempfile::tempdir().unwrap();
        let em = EdgeManager::new(home.path());
        let net = FakeShareNet::default();
        let c = conn();
        em.ensure(&net, &c, OWNER, 10, edge_plan).await.unwrap();
        // An orphan: deployed by a crashed earlier run, not recorded.
        let mut orphan_plan = edge_plan("t");
        orphan_plan.tags.insert(tag::OWNER.into(), OWNER.into());
        net.deploy_edge(&c, &orphan_plan).await.unwrap();
        // Shares are live and it was used recently: only the orphan goes.
        let gone = em
            .sweep(&net, OWNER, std::slice::from_ref(&c), true, 20, 100)
            .await;
        assert_eq!(gone.len(), 1);
        assert_eq!(net.edges.lock().unwrap().len(), 1);
        // Idle past the limit with no live share: removed.
        let gone = em
            .sweep(&net, OWNER, std::slice::from_ref(&c), false, 10 + 101, 100)
            .await;
        assert_eq!(gone.len(), 1);
        assert!(net.edges.lock().unwrap().is_empty());
        assert!(em.record().edge.is_none());
    }

    #[tokio::test]
    async fn touching_renews_an_edge_near_its_expiry() {
        let home = tempfile::tempdir().unwrap();
        let em = EdgeManager::new(home.path());
        let net = FakeShareNet::default();
        let c = conn();
        let (e, _) = em.ensure(&net, &c, OWNER, 10, edge_plan).await.unwrap();
        em.touch(&net, 20).await.unwrap();
        assert!(
            !net.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("renew"))
        );
        em.touch(&net, e.expires - 3600).await.unwrap();
        assert!(
            net.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("renew"))
        );
        assert!(em.record().edge.unwrap().expires > e.expires);
    }
}
