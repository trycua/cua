// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Public TCP shares through the owner's own cloud (CUA-2602), as the cua
//! daemon drives them: which route a share takes ([`CloudShares::plan`]),
//! the host route's firewall rule ([`CloudShares::open_host`]), the share
//! edge ([`CloudShares::ensure_edge`]) and the sweep that leaves nothing
//! behind ([`CloudShares::sweep`]).
//!
//! Everything cloud-side goes through [`ShareNetApi`], so the whole flow is
//! tested against [`crate::share_net::FakeShareNet`]. The daemon adds what
//! needs a Space or the relay: publishing the slot in the Space, joining the
//! edge, and the share records.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cua_sandbox_core::Error;

use crate::api::Result;
use crate::model::Connection;
use crate::share_edge::{EDGE_IDLE_SECS, EdgeDeployed, EdgeInputs};
use crate::share_net::{
    EdgeManager, HOST_SLOTS, OpenRequest, RuleManager, ShareNetApi, space_machine,
};
use crate::share_route::{
    LinkedCloud, NoRoute, RoutePlan, RouteRequest, ShareSettings, SpaceLocation, choose,
};
use crate::state::Store;

/// The plain name of a cloud (`AWS`).
pub fn cloud_title(provider: &str) -> String {
    match provider {
        "aws" => "AWS".into(),
        "gcp" => "Google Cloud".into(),
        "azure" => "Azure".into(),
        "hetzner" => "Hetzner".into(),
        "modal" => "Modal".into(),
        other => other.to_owned(),
    }
}

/// A host-route share as opened: where people connect, and what the daemon
/// must publish in the Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostOpened {
    /// The cloud word.
    pub provider: String,
    /// The computer's public address.
    pub ip: String,
    /// The share slot (the public port; the Space's spacesd serves it).
    pub slot: u16,
    /// The firewall rule.
    pub rule: String,
}

/// What a sweep removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Firewall rules closed.
    pub rules: Vec<String>,
    /// Share edges removed.
    pub edges: Vec<String>,
}

/// Public TCP shares in the owner's clouds.
pub struct CloudShares {
    home: PathBuf,
    store: Store,
    rules: RuleManager,
    edges: EdgeManager,
    nets: Vec<Arc<dyn ShareNetApi>>,
}

impl CloudShares {
    /// The shares of cua home `home`, over the clouds in `nets`.
    pub fn new(home: &Path, nets: Vec<Arc<dyn ShareNetApi>>) -> Self {
        Self {
            home: home.to_path_buf(),
            store: Store::new(home),
            rules: RuleManager::new(home),
            edges: EdgeManager::new(home),
            nets,
        }
    }

    /// The clouds this build can open shares in.
    #[allow(clippy::vec_init_then_push)]
    pub fn builtin(home: &Path) -> Self {
        #[allow(unused_mut)]
        let mut nets: Vec<Arc<dyn ShareNetApi>> = Vec::new();
        #[cfg(feature = "aws")]
        nets.push(Arc::new(crate::aws::Aws::new()));
        Self::new(home, nets)
    }

    fn net(&self, provider: &str) -> Option<Arc<dyn ShareNetApi>> {
        self.nets.iter().find(|n| n.provider() == provider).cloned()
    }

    fn io(e: std::io::Error) -> Error {
        Error::Cloud(e.to_string())
    }

    fn owner(&self) -> Result<String> {
        self.store.owner_id().map_err(Self::io)
    }

    fn conn(&self, provider: &str) -> Result<Connection> {
        self.store
            .connection(provider)
            .map_err(Self::io)?
            .ok_or_else(|| {
                Error::InvalidArgument(format!("{} is not linked", cloud_title(provider)))
            })
    }

    /// The sharing settings.
    pub fn settings(&self) -> ShareSettings {
        ShareSettings::load(&self.home)
    }

    /// Linked clouds, and whether each can hold a share edge here.
    pub fn linked(&self) -> Vec<LinkedCloud> {
        self.store
            .connections()
            .unwrap_or_default()
            .into_iter()
            .map(|c| LinkedCloud {
                can_edge: self.net(&c.provider).is_some(),
                title: cloud_title(&c.provider),
                provider: c.provider,
            })
            .collect()
    }

    /// Where `space` (its qualified id, `aws:mc`) runs.
    pub fn location(&self, space: &str) -> SpaceLocation {
        let resources = self.store.resources().unwrap_or_default();
        match space_machine(&resources, space) {
            Some(m) => SpaceLocation::Cloud {
                can_publish: self.net(&m.provider).is_some(),
                provider: m.provider,
            },
            None => SpaceLocation::Local,
        }
    }

    /// The route a share of `space` takes.
    pub fn plan(
        &self,
        space: &str,
        asked: RouteRequest,
        settings: &ShareSettings,
    ) -> std::result::Result<RoutePlan, NoRoute> {
        choose(&self.location(space), &self.linked(), settings, asked)
    }

    /// A share slot no open rule on `space`'s firewall holds.
    pub fn free_slot(&self, space: &str) -> Option<u16> {
        let resources = self.store.resources().unwrap_or_default();
        let m = space_machine(&resources, space)?;
        let used: HashSet<u16> = self
            .rules
            .list()
            .into_iter()
            .filter(|r| r.group == m.group)
            .map(|r| r.port)
            .collect();
        (HOST_SLOTS.0..=HOST_SLOTS.1).find(|p| !used.contains(p))
    }

    /// Opens the host route of share `share`: one slot of `space`'s cloud
    /// machine, to `cidrs` (empty: anyone), until `expires` (unix seconds).
    /// The daemon then publishes the slot in the Space; if that fails it
    /// calls [`CloudShares::close`].
    pub async fn open_host(
        &self,
        share: &str,
        space: &str,
        cidrs: &[String],
        expires: u64,
    ) -> Result<HostOpened> {
        let resources = self.store.resources().map_err(Self::io)?;
        let m = space_machine(&resources, space).ok_or_else(|| {
            Error::InvalidArgument("this Space does not run on its own host in your cloud".into())
        })?;
        let api = self.net(&m.provider).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{} cannot open share ports from this build",
                cloud_title(&m.provider)
            ))
        })?;
        let conn = self.conn(&m.provider)?;
        let owner = self.owner()?;
        let ip = api
            .machine_address(&conn, &m.instance)
            .await?
            .ok_or_else(|| Error::Cloud("the Space's host has no public address".into()))?;
        let slot = self.free_slot(space).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "every share slot of this Space's host is in use ({}-{}); stop a share first",
                HOST_SLOTS.0, HOST_SLOTS.1
            ))
        })?;
        let rec = self
            .rules
            .open(
                api.as_ref(),
                &OpenRequest {
                    share,
                    owner: &owner,
                    conn: &conn,
                    group: &m.group,
                    group_tags: &m.group_tags,
                    port: slot,
                    cidrs,
                    expires,
                },
            )
            .await?;
        Ok(HostOpened {
            provider: m.provider,
            ip,
            slot,
            rule: rec.rule,
        })
    }

    /// Closes share `share`'s firewall rule, if it has one.
    pub async fn close(&self, share: &str) -> Result<()> {
        let Some(r) = self.rules.list().into_iter().find(|r| r.share == share) else {
            return Ok(());
        };
        let Some(api) = self.net(&r.conn.provider) else {
            return Ok(());
        };
        self.rules.close(api.as_ref(), share).await
    }

    /// The share edge in `provider` (made the first time, then reused),
    /// with the token it accepts. The owner must have agreed to it
    /// ([`ShareSettings::edge_consent`]); the daemon asks first.
    pub async fn ensure_edge(
        &self,
        provider: &str,
        version: &str,
        now: u64,
    ) -> Result<(EdgeDeployed, String)> {
        if !self.settings().edge_consent.iter().any(|p| p == provider) {
            return Err(Error::InvalidArgument(format!(
                "a share edge in {} needs your agreement first",
                cloud_title(provider)
            )));
        }
        let api = self.net(provider).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{} cannot hold a share edge from this build",
                cloud_title(provider)
            ))
        })?;
        let conn = self.conn(provider)?;
        let owner = self.owner()?;
        let region = conn.region.clone();
        self.edges
            .ensure(api.as_ref(), &conn, &owner, now, |token| {
                crate::share_edge::plan(&EdgeInputs {
                    provider,
                    region: &region,
                    owner: &owner,
                    token,
                    version,
                    now,
                })
            })
            .await
    }

    /// The edge as recorded, if there is one.
    pub fn edge(&self) -> Option<EdgeDeployed> {
        self.edges.record().edge
    }

    /// Notes that a share used the edge (renews it near its expiry).
    pub async fn touch_edge(&self, now: u64) -> Result<()> {
        let Some(c) = self.edges.record().conn else {
            return Ok(());
        };
        let Some(api) = self.net(&c.provider) else {
            return Ok(());
        };
        self.edges.touch(api.as_ref(), now).await
    }

    /// Removes the share edge now (Settings).
    pub async fn remove_edge(&self) -> Result<()> {
        let Some(c) = self.edges.record().conn else {
            return Ok(());
        };
        let Some(api) = self.net(&c.provider) else {
            return Ok(());
        };
        self.edges.remove(api.as_ref()).await
    }

    /// Closes the rules of shares not in `live` (and expired or orphaned
    /// ones), and removes the edge when it is idle (no live share on it for
    /// [`EDGE_IDLE_SECS`]), expired or orphaned. Only Cua-tagged parts of
    /// this home are ever touched.
    pub async fn sweep(&self, live: &HashSet<String>, edge_live: bool, now: u64) -> SweepReport {
        let mut out = SweepReport::default();
        // Nothing linked, nothing to sweep. Checked before the owner id:
        // reading that id creates it, and a sweep must not write the state
        // file (it would bring back a deleted home, see the daemon's home
        // watchdog).
        let conns = self.store.connections().unwrap_or_default();
        if conns.is_empty() {
            return out;
        }
        let Ok(owner) = self.owner() else {
            return out;
        };
        for api in &self.nets {
            let mine: Vec<Connection> = conns
                .iter()
                .filter(|c| c.provider == api.provider())
                .cloned()
                .collect();
            if mine.is_empty() {
                continue;
            }
            out.rules.extend(
                self.rules
                    .sweep(api.as_ref(), &owner, &mine, live, now)
                    .await,
            );
            out.edges.extend(
                self.edges
                    .sweep(api.as_ref(), &owner, &mine, edge_live, now, EDGE_IDLE_SECS)
                    .await,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Resource;
    use crate::share_net::FakeShareNet;
    use crate::share_route::Route;
    use std::collections::BTreeMap;

    struct Rig {
        _dir: tempfile::TempDir,
        net: Arc<FakeShareNet>,
        cs: CloudShares,
    }

    /// A home with AWS linked and Space `aws:mc` on its own machine.
    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store
            .connect(Connection {
                provider: "aws".into(),
                region: "us-west-2".into(),
                ..Default::default()
            })
            .unwrap();
        let owner = store.owner_id().unwrap();
        let mut tags = BTreeMap::new();
        tags.insert(crate::model::tag::MANAGED.to_owned(), "true".to_owned());
        tags.insert(crate::model::tag::OWNER.to_owned(), owner);
        store
            .record(Resource {
                provider: "aws".into(),
                id: "sg-1".into(),
                resource_type: "security_group".into(),
                tags,
                ..Default::default()
            })
            .unwrap();
        let mut inst = Resource {
            provider: "aws".into(),
            id: "i-0abc".into(),
            resource_type: "instance".into(),
            sandbox: "aws:mc".into(),
            ..Default::default()
        };
        inst.extra.insert("security_group".into(), "sg-1".into());
        store.record(inst).unwrap();
        let net = Arc::new(FakeShareNet::default());
        let cs = CloudShares::new(dir.path(), vec![net.clone() as Arc<dyn ShareNetApi>]);
        Rig { _dir: dir, net, cs }
    }

    #[tokio::test]
    async fn a_sweep_with_no_cloud_linked_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let net = Arc::new(FakeShareNet::default());
        let cs = CloudShares::new(&home, vec![net as Arc<dyn ShareNetApi>]);
        let report = cs.sweep(&HashSet::new(), false, 1).await;
        assert!(report.rules.is_empty() && report.edges.is_empty());
        // No owner id was minted: a deleted home stays deleted.
        assert!(!home.exists());
    }

    #[test]
    fn a_cloud_space_takes_the_host_route_and_a_local_one_the_edge() {
        let r = rig();
        let s = ShareSettings::default();
        let p = r.cs.plan("aws:mc", RouteRequest::Auto, &s).unwrap();
        assert_eq!(p.route, Route::Host);
        assert_eq!(p.line(), "Through your AWS account");
        let p =
            r.cs.plan("local:laptop-game", RouteRequest::Auto, &s)
                .unwrap();
        assert_eq!(p.route, Route::Edge);
        assert!(p.needs_consent, "the first edge asks");
        // Without the cloud's share support in this build: the relay.
        let bare = CloudShares::new(r._dir.path(), vec![]);
        let p = bare.plan("aws:mc", RouteRequest::Auto, &s).unwrap();
        assert_eq!(p.route, Route::Relay);
    }

    #[tokio::test]
    async fn the_host_route_opens_one_tagged_slot_per_share_and_closes_it() {
        let r = rig();
        let a =
            r.cs.open_host("share-a", "aws:mc", &[], 9_999_999_999)
                .await
                .unwrap();
        assert_eq!(a.slot, HOST_SLOTS.0);
        assert!(a.ip.starts_with("203.0.113."));
        let b =
            r.cs.open_host(
                "share-b",
                "aws:mc",
                &["198.51.100.0/24".into()],
                9_999_999_999,
            )
            .await
            .unwrap();
        assert_eq!(b.slot, HOST_SLOTS.0 + 1, "the next free slot");
        {
            let rules = r.net.rules.lock().unwrap();
            assert_eq!(rules.len(), 2);
            assert!(rules.iter().all(|x| x.group == "sg-1"));
            assert_eq!(rules[0].cidrs, vec!["0.0.0.0/0"]);
            assert_eq!(rules[1].cidrs, vec!["198.51.100.0/24"]);
        }
        r.cs.close("share-a").await.unwrap();
        assert_eq!(r.net.rules.lock().unwrap().len(), 1);
        // The freed slot is reused.
        let c =
            r.cs.open_host("share-c", "aws:mc", &[], 9_999_999_999)
                .await
                .unwrap();
        assert_eq!(c.slot, HOST_SLOTS.0);
        // A Space not in the cloud has no host route.
        assert!(
            r.cs.open_host("share-d", "local:x", &[], 9_999_999_999)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn every_slot_in_use_is_a_clear_error() {
        let r = rig();
        for i in 0..=(HOST_SLOTS.1 - HOST_SLOTS.0) {
            r.cs.open_host(&format!("s-{i}"), "aws:mc", &[], 9_999_999_999)
                .await
                .unwrap();
        }
        let e =
            r.cs.open_host("one-more", "aws:mc", &[], 9_999_999_999)
                .await
                .unwrap_err();
        assert!(e.to_string().contains("every share slot"), "{e}");
    }

    #[tokio::test]
    async fn an_edge_needs_consent_then_is_made_once() {
        let r = rig();
        assert!(
            r.cs.ensure_edge("aws", "1.0.0", 100).await.is_err(),
            "no consent yet"
        );
        assert!(r.net.edges.lock().unwrap().is_empty());
        let mut s = ShareSettings::default();
        s.edge_consent.push("aws".into());
        s.save(r._dir.path()).unwrap();
        let (e1, t1) = r.cs.ensure_edge("aws", "1.0.0", 100).await.unwrap();
        let (e2, t2) = r.cs.ensure_edge("aws", "1.0.0", 200).await.unwrap();
        assert_eq!((e1.instance.clone(), t1.clone()), (e2.instance, t2));
        assert_eq!(r.net.edges.lock().unwrap().len(), 1);
        assert_eq!(r.cs.edge().unwrap().instance, e1.instance);
        r.cs.remove_edge().await.unwrap();
        assert!(r.net.edges.lock().unwrap().is_empty());
        assert!(r.cs.edge().is_none());
    }

    #[tokio::test]
    async fn after_a_crash_the_sweep_leaves_nothing_behind() {
        let r = rig();
        r.cs.open_host("share-live", "aws:mc", &[], 9_999_999_999)
            .await
            .unwrap();
        r.cs.open_host("share-dead", "aws:mc", &[], 9_999_999_999)
            .await
            .unwrap();
        let mut s = ShareSettings::default();
        s.edge_consent.push("aws".into());
        s.save(r._dir.path()).unwrap();
        r.cs.ensure_edge("aws", "1.0.0", 100).await.unwrap();
        // The daemon "crashed": share-dead is gone from its records; a new
        // daemon sweeps with only share-live live and no edge share.
        let live: HashSet<String> = ["share-live".to_string()].into();
        let rep = r.cs.sweep(&live, false, 100 + EDGE_IDLE_SECS + 1).await;
        assert_eq!(rep.rules.len(), 1);
        assert_eq!(rep.edges.len(), 1, "the idle edge goes");
        {
            let rules = r.net.rules.lock().unwrap();
            assert_eq!(rules.len(), 1);
            assert!(r.net.edges.lock().unwrap().is_empty());
        }
        // Nothing live: everything goes.
        let rep =
            r.cs.sweep(&HashSet::new(), false, 100 + EDGE_IDLE_SECS + 2)
                .await;
        assert_eq!(rep.rules.len(), 1);
        assert!(r.net.rules.lock().unwrap().is_empty());
    }
}
