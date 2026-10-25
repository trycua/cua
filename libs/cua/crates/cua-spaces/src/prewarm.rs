// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Prewarm: keep `count` Spaces of one shape ready on the machines the
//! elastic pool picks, so the next creates of that shape start in a fraction
//! of a second instead of booting (or waiting for a machine).
//!
//! - **Where** is the same solve as every placement ([`crate::elastic`]):
//!   your devices first, then hosts with room, then the cheapest new
//!   machines, under the budget, quotas and pins. A prewarm is pinned
//!   demand: what it holds counts as taken on its machines, and those
//!   machines are not scaled in while it lives.
//! - **How**: each machine keeps a named warm pool (`prewarm-<id>`, the cua
//!   daemon's own pool machinery, [`LocalPools`]): members are leased,
//!   hidden, and claimed by rename and token rotation, so a claimed Space is
//!   a cold one in every respect but its start time. New hosts are added
//!   with the image pulled. A remote machine is asked through
//!   `HostSpacesService.SetHostWarm`; this one directly.
//! - **Claim**: any create of the same shape (image, kind, CPUs, memory) on
//!   such a machine claims a member, through every surface; the pool starts
//!   a replacement, so `count` stay ready until the TTL.
//! - **End**: at its TTL (default an hour) or on [`Spaces::remove_prewarm`]
//!   the pools go (members deleted) and hosts added for it are removed by
//!   the scaler at their billing boundary.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use cua_cluster::elastic::Target;
use cua_proto::env::v1 as pb;
use cua_sandbox_core::placement::{Kind, On};
use serde::{Deserialize, Serialize};

use crate::elastic::ElasticRequest;
use crate::{Error, Result, SpaceCreate, Spaces};

/// TTL when none is asked for.
pub const DEFAULT_TTL_SECS: u64 = 3600;
/// The longest a prewarm lives.
pub const MAX_TTL_SECS: u64 = 7 * 24 * 3600;
/// The most Spaces one prewarm keeps ready.
pub const MAX_COUNT: u32 = 256;

/// This machine's warm pools (the cua daemon's): what a prewarm asks a
/// machine to keep.
pub trait LocalPools: Send + Sync + 'static {
    /// Keeps `r.count` members of the shape in pool `r.name` (0 removes it).
    fn set_warm<'a>(
        &'a self,
        r: &'a pb::SetHostWarmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<pb::SetHostWarmResponse>> + Send + 'a>>;
}

/// A prewarm to start.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PrewarmRequest {
    /// Image (`linux`, an OCI reference).
    pub image: String,
    /// Spaces kept ready (1 to [`MAX_COUNT`]).
    pub count: u32,
    /// `auto` (default), `local`, a linked cloud or `host:<machine>`.
    #[serde(default)]
    pub on: Option<String>,
    /// `container`, `vm` or `microvm` (default: the image's).
    #[serde(default)]
    pub kind: Option<String>,
    /// vCPUs each (default: the size detected for the image).
    #[serde(default)]
    pub cpus: Option<u32>,
    /// MiB each.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// Seconds it lives (default [`DEFAULT_TTL_SECS`]).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// Pin: the cloud machine type.
    #[serde(default)]
    pub instance_type: Option<String>,
    /// Pin: the cloud region.
    #[serde(default)]
    pub region: Option<String>,
}

/// Where a prewarm keeps its Spaces.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrewarmPlacement {
    /// The machine's name (`this machine`, `aws-3`).
    pub host: String,
    /// How to reach it: `local` or its relay machine id.
    pub machine: String,
    /// Spaces kept ready there.
    pub count: u32,
    /// Ready there when last asked.
    pub ready: u32,
}

/// A prewarm.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prewarm {
    /// `pw-xxxxxx`.
    pub id: String,
    /// The shape its Spaces have.
    pub image: String,
    pub kind: String,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Asked.
    pub count: u32,
    /// Ready now, all machines.
    pub ready: u32,
    pub placements: Vec<PrewarmPlacement>,
    /// Unix seconds.
    pub expires_at: u64,
    /// Hosts added for it.
    #[serde(default)]
    pub new_hosts: Vec<String>,
}

impl Prewarm {
    /// The pool name on each machine.
    pub fn pool_name(&self) -> String {
        format!("prewarm-{}", self.id.trim_start_matches("pw-"))
    }

    /// Whether a create of this shape claims its Spaces.
    pub fn matches(&self, image: &str, kind: &str, cpus: u32, memory_mb: u64) -> bool {
        self.image == image
            && (self.kind == kind || kind.is_empty())
            && self.cpus == cpus
            && self.memory_mb == memory_mb
    }

    fn request(&self, count: u32) -> pb::SetHostWarmRequest {
        pb::SetHostWarmRequest {
            name: self.pool_name(),
            image: self.image.clone(),
            kind: self.kind.clone(),
            runtime: String::new(),
            cpus: self.cpus,
            memory_mb: self.memory_mb,
            count,
            ttl_secs: self.expires_at.saturating_sub(now()).max(60),
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file(home: &Path) -> std::path::PathBuf {
    home.join("elastic").join("prewarm.json")
}

/// The prewarms of `home` (expired ones included until they are ended).
pub fn load(home: &Path) -> Vec<Prewarm> {
    std::fs::read(file(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(home: &Path, all: &[Prewarm]) -> Result<()> {
    let p = file(home);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(all)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// Checks a request; returns its TTL.
pub fn validate(r: &PrewarmRequest) -> Result<u64> {
    if r.image.trim().is_empty() {
        return Err(Error::invalid("prewarm: an image is needed"));
    }
    if r.count == 0 || r.count > MAX_COUNT {
        return Err(Error::invalid(format!(
            "prewarm: count {} (1 to {MAX_COUNT})",
            r.count
        )));
    }
    let ttl = r.ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
    if ttl == 0 || ttl > MAX_TTL_SECS {
        return Err(Error::invalid(format!(
            "prewarm: ttl {ttl} s (1 s to {MAX_TTL_SECS} s)"
        )));
    }
    Ok(ttl)
}

/// Spaces each live prewarm holds per machine name (what the scaler must
/// not take away and what claims free up), for the shape given (`None`:
/// every shape).
pub fn held(
    all: &[Prewarm],
    t: u64,
    shape: Option<(&str, &str, u32, u64)>,
) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for p in all.iter().filter(|p| p.expires_at > t) {
        if let Some((image, kind, cpus, mem)) = shape
            && !p.matches(image, kind, cpus, mem)
        {
            continue;
        }
        for pl in &p.placements {
            *out.entry(pl.host.clone()).or_default() += pl.count;
        }
    }
    out
}

impl Spaces {
    /// Installs this machine's warm pools (the cua daemon does).
    pub fn set_local_pools(&self, pools: Option<Arc<dyn LocalPools>>) {
        *self.inner.elastic.local_pools.write().unwrap() = pools;
    }

    pub(crate) fn local_pools(&self) -> Option<Arc<dyn LocalPools>> {
        self.inner.elastic.local_pools.read().unwrap().clone()
    }

    /// Keeps `r.count` Spaces of one shape ready where the elastic pool
    /// puts them (see the module docs). Refused, with the solver's reason,
    /// when the budget, quotas or pins cannot fit it; nothing is bought then.
    pub async fn prewarm(&self, r: PrewarmRequest) -> Result<Prewarm> {
        let ttl = validate(&r)?;
        let on = match r.on.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
            None | Some("auto") => On::Auto,
            Some(o) => On::parse_or_host(o).map_err(|e| Error::invalid(e.message))?,
        };
        let kind = match r.kind.as_deref() {
            None | Some("") => Kind::Auto,
            Some(k) => Kind::parse(k).map_err(|e| Error::invalid(e.message))?,
        };
        let opts = SpaceCreate {
            image: Some(r.image.clone()),
            on: Some(on.clone()),
            kind,
            cpus: r.cpus,
            memory_mb: r.memory_mb,
            instance_type: r.instance_type.clone(),
            region: r.region.clone(),
            ..Default::default()
        };
        let mut req: ElasticRequest = crate::elastic::request_of(&opts);
        req.count = r.count;
        req.caller = cua_cluster::elastic::detect::Caller::Batch;
        req.duration_secs = Some(ttl);
        match &on {
            On::Local => req.pins.devices_only = true,
            On::Host(h) => req.pin_host = h.clone(),
            _ => {}
        }
        // The same size a create of this shape books, so it claims these.
        let g = crate::elastic::detect_for(&req, &self.usage_book());
        let cpus = r
            .cpus
            .unwrap_or(g.per.millicpus.div_ceil(1000).max(1) as u32);
        let memory_mb = r.memory_mb.unwrap_or(g.per.memory_mb);
        req.cpus = Some(cpus);
        req.memory_mb = Some(memory_mb);
        let image = crate::elastic::canonical_image_of(&r.image);
        let id = format!("pw-{:06x}", rand::random::<u32>() & 0xff_ffff);
        // One solve and its adds, like a create.
        let (targets, new_hosts) = {
            let _g = self.inner.elastic.placing.lock().await;
            let p = self.elastic_plan(std::slice::from_ref(&req)).await?;
            if let Some(u) = p.plan.unplaced.first() {
                return Err(Error::LimitExceeded(format!(
                    "prewarm {} x {}: {} cannot be placed: {}",
                    r.count, r.image, u.count, u.reason
                )));
            }
            // Spaces kept ready in your cloud each use a live session once
            // claimed: no machine is bought for more than the plan allows.
            let in_cloud: u32 = p
                .plan
                .assignments
                .iter()
                .filter(|a| crate::elastic::lands_in_cloud(a, &p.pool))
                .map(|a| a.count)
                .sum();
            if in_cloud > 0
                && let Some(room) = self.cloud_session_room().await
                && in_cloud > room.left
            {
                return Err(room.refusal(in_cloud));
            }
            let added = if p
                .plan
                .assignments
                .iter()
                .any(|a| matches!(a.target, Target::New { .. }))
            {
                self.elastic_apply_pulling(&p.plan, std::slice::from_ref(&image))
                    .await?
            } else {
                vec![]
            };
            let mut targets = Vec::new();
            for a in &p.plan.assignments {
                targets.push((
                    crate::elastic::resolve_for(a, &p.plan, &added, &p.pool),
                    a.count,
                ));
            }
            let new_hosts: Vec<String> = added.iter().flatten().map(|h| h.name.clone()).collect();
            (targets, new_hosts)
        };
        let mut pw = Prewarm {
            id,
            image,
            kind: match kind {
                Kind::Auto => String::new(),
                k => k.as_str().to_string(),
            },
            cpus,
            memory_mb,
            count: r.count,
            ready: 0,
            placements: vec![],
            expires_at: now() + ttl,
            new_hosts,
        };
        let mut errors = Vec::new();
        for (t, count) in targets {
            match self.prewarm_target(&pw, t, count).await {
                Ok(pl) => pw.placements.push(pl),
                Err(e) => errors.push(e.to_string()),
            }
        }
        if pw.placements.is_empty() {
            return Err(Error::LimitExceeded(format!(
                "prewarm {} x {}: no machine took it ({})",
                r.count,
                r.image,
                errors.join("; ")
            )));
        }
        pw.ready = pw.placements.iter().map(|p| p.ready).sum();
        let mut all = load(self.home_dir());
        all.push(pw.clone());
        save(self.home_dir(), &all)?;
        Ok(pw)
    }

    async fn prewarm_target(
        &self,
        pw: &Prewarm,
        t: crate::elastic::PlacedTarget,
        count: u32,
    ) -> Result<PrewarmPlacement> {
        let (host, machine) = match t {
            crate::elastic::PlacedTarget::Host { name, id } => (name, id),
            crate::elastic::PlacedTarget::NewHost(h) => {
                let m = self.wait_joined_pub(&h).await?;
                (h.name, m)
            }
            crate::elastic::PlacedTarget::Other(why) => return Err(Error::invalid(why)),
        };
        let resp = self.set_warm_on(&machine, &pw.request(count)).await?;
        Ok(PrewarmPlacement {
            host,
            machine,
            count,
            ready: resp.ready,
        })
    }

    /// Asks machine `machine` (`local`, or a relay machine id) to keep the
    /// pool `r` describes.
    async fn set_warm_on(
        &self,
        machine: &str,
        r: &pb::SetHostWarmRequest,
    ) -> Result<pb::SetHostWarmResponse> {
        if machine == "local" {
            let pools = self.local_pools().ok_or_else(|| {
                Error::host(
                    "prewarm",
                    "this machine keeps no warm pools here (run it in the cua daemon)",
                )
            })?;
            return pools.set_warm(r).await;
        }
        let host = Box::pin(self.space(&format!("relay:{machine}"))).await?;
        let label = self.host_label(machine).await;
        crate::host_spaces::guard_host_rpc(&host, &label, "warm pools")?;
        host.spacesd()?
            .host_spaces()
            .set_host_warm(r.clone())
            .await
            .map(|r| r.into_inner())
            .map_err(|e| crate::host_spaces::from_new_host_rpc(e, &label, "warm pools"))
    }

    /// The prewarms of this home, with how many are ready on each machine
    /// now (machines that do not answer keep their last count).
    pub async fn prewarms(&self) -> Result<Vec<Prewarm>> {
        let mut all = load(self.home_dir());
        let t = now();
        for p in all.iter_mut().filter(|p| p.expires_at > t) {
            for i in 0..p.placements.len() {
                let pl = p.placements[i].clone();
                if let Ok(r) = self.set_warm_on(&pl.machine, &p.request(pl.count)).await {
                    p.placements[i].ready = r.ready;
                }
            }
            p.ready = p.placements.iter().map(|p| p.ready).sum();
        }
        Ok(all)
    }

    /// Ends prewarm `id`: its pools and their ready Spaces go. `false`: no
    /// such prewarm.
    pub async fn remove_prewarm(&self, id: &str) -> Result<bool> {
        let all = load(self.home_dir());
        let Some(p) = all.iter().find(|p| p.id == id).cloned() else {
            return Ok(false);
        };
        self.end_prewarm(&p).await;
        let mut all = load(self.home_dir());
        all.retain(|x| x.id != id);
        save(self.home_dir(), &all)?;
        Ok(true)
    }

    async fn end_prewarm(&self, p: &Prewarm) {
        for pl in &p.placements {
            if let Err(e) = self.set_warm_on(&pl.machine, &p.request(0)).await {
                // The pool on that machine ends by itself (it keeps none
                // after its TTL without a claim).
                tracing::warn!(host = pl.host, "prewarm pool not removed: {e}");
            }
        }
    }

    /// Ends the prewarms past their TTL (the daemon's reconcile loop).
    pub async fn prewarm_reconcile(&self) -> Result<u32> {
        let t = now();
        let all = load(self.home_dir());
        let (done, live): (Vec<_>, Vec<_>) = all.into_iter().partition(|p| p.expires_at <= t);
        for p in &done {
            self.end_prewarm(p).await;
        }
        if !done.is_empty() {
            save(self.home_dir(), &live)?;
        }
        Ok(done.len() as u32)
    }
}

impl crate::host_spaces::HostSpacesServer {
    /// `HostSpacesService.SetHostWarm`: keeps a warm pool on this machine for
    /// the owner's prewarm.
    pub async fn set_warm(
        &self,
        caller: &crate::host_spaces::HostCaller,
        r: pb::SetHostWarmRequest,
    ) -> Result<pb::SetHostWarmResponse> {
        if !caller.is_owner() {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(
                "only this machine's owner keeps Spaces ready on it".into(),
            )));
        }
        if !r.name.starts_with("prewarm-") {
            return Err(Error::invalid(format!(
                "warm pool {:?}: prewarm pools are named prewarm-<id>",
                r.name
            )));
        }
        if r.count > 0 && !self.provides_spaces() {
            return Err(Error::host(
                "host spaces",
                "this machine does not provide Spaces (on it: `cua host config --provide-spaces on`)",
            ));
        }
        let pools = self.spaces_ref().local_pools().ok_or_else(|| {
            Error::host("prewarm", "this machine's cua daemon keeps no warm pools")
        })?;
        pools.set_warm(&r).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw(id: &str, host: &str, n: u32, expires_at: u64) -> Prewarm {
        Prewarm {
            id: id.into(),
            image: "ghcr.io/trycua/linux:24.04-slim".into(),
            kind: "container".into(),
            cpus: 1,
            memory_mb: 512,
            count: n,
            placements: vec![PrewarmPlacement {
                host: host.into(),
                machine: "m".into(),
                count: n,
                ready: 0,
            }],
            expires_at,
            ..Default::default()
        }
    }

    #[test]
    fn requests_are_checked() {
        let ok = PrewarmRequest {
            image: "linux".into(),
            count: 8,
            ..Default::default()
        };
        assert_eq!(validate(&ok).unwrap(), DEFAULT_TTL_SECS);
        for bad in [
            PrewarmRequest {
                count: 0,
                ..ok.clone()
            },
            PrewarmRequest {
                count: MAX_COUNT + 1,
                ..ok.clone()
            },
            PrewarmRequest {
                image: " ".into(),
                ..ok.clone()
            },
            PrewarmRequest {
                ttl_secs: Some(0),
                ..ok.clone()
            },
            PrewarmRequest {
                ttl_secs: Some(MAX_TTL_SECS + 1),
                ..ok.clone()
            },
        ] {
            assert!(validate(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn live_prewarms_hold_their_machines_and_expired_ones_do_not() {
        let all = vec![
            pw("pw-1", "aws-1", 8, 2000),
            pw("pw-2", "aws-1", 4, 500),
            pw("pw-3", "aws-2", 2, 2000),
        ];
        let h = held(&all, 1000, None);
        assert_eq!(h.get("aws-1"), Some(&8));
        assert_eq!(h.get("aws-2"), Some(&2));
        // Only the shape asked.
        let h = held(
            &all,
            1000,
            Some(("ghcr.io/trycua/linux:24.04-slim", "container", 1, 1024)),
        );
        assert!(h.is_empty());
        let h = held(
            &all,
            1000,
            Some(("ghcr.io/trycua/linux:24.04-slim", "", 1, 512)),
        );
        assert_eq!(h.values().sum::<u32>(), 10);
        assert_eq!(all[0].pool_name(), "prewarm-1");
    }

    struct Pools(std::sync::Mutex<Vec<(String, u32)>>);

    impl LocalPools for Pools {
        fn set_warm<'a>(
            &'a self,
            r: &'a pb::SetHostWarmRequest,
        ) -> Pin<Box<dyn Future<Output = Result<pb::SetHostWarmResponse>> + Send + 'a>> {
            Box::pin(async move {
                self.0.lock().unwrap().push((r.name.clone(), r.count));
                Ok(pb::SetHostWarmResponse {
                    ready: r.count,
                    starting: 0,
                    reason: String::new(),
                })
            })
        }
    }

    #[tokio::test]
    async fn a_local_prewarm_keeps_a_pool_and_its_ttl_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spaces::builder().home(dir.path()).build();
        let pools = Arc::new(Pools(Default::default()));
        s.set_local_pools(Some(pools.clone()));
        // Recorded as if placed on this machine (the solve is covered in
        // cua-cluster); the pool is asked for, then removed at its TTL.
        let mut p = pw("pw-abc123", "this machine", 3, now() + 3600);
        p.placements[0].machine = "local".into();
        let r = s.set_warm_on("local", &p.request(3)).await.unwrap();
        assert_eq!(r.ready, 3);
        p.expires_at = now() - 1;
        save(dir.path(), &[p.clone()]).unwrap();
        assert_eq!(s.prewarm_reconcile().await.unwrap(), 1);
        assert!(load(dir.path()).is_empty());
        assert_eq!(
            *pools.0.lock().unwrap(),
            vec![
                ("prewarm-abc123".to_string(), 3),
                ("prewarm-abc123".to_string(), 0)
            ]
        );
        // A live one is listed and removed on request.
        let mut q = pw("pw-def456", "this machine", 2, now() + 3600);
        q.placements[0].machine = "local".into();
        save(dir.path(), &[q]).unwrap();
        assert_eq!(s.prewarms().await.unwrap()[0].ready, 2);
        assert!(s.remove_prewarm("pw-def456").await.unwrap());
        assert!(!s.remove_prewarm("pw-def456").await.unwrap());
        assert!(load(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_prewarm_the_budget_cannot_fit_buys_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spaces::builder().home(dir.path()).build();
        // No devices that provide Spaces, no clouds: nothing can hold it.
        let e = s
            .prewarm(PrewarmRequest {
                image: "ghcr.io/trycua/linux:24.04-slim".into(),
                count: 4,
                on: Some("aws".into()),
                kind: Some("container".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(e.to_string().contains("cannot be placed"), "{e}");
        assert!(load(dir.path()).is_empty());
    }
}
