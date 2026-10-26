// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`Clouds`]: the [`CloudManager`] behind `cua cloud` (and the Spaces
//! `cloud_*` tools): status, test, connect, disconnect and sweep over every
//! [`CloudApi`] this build includes, with the state of one cua home.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{
    AddHost, CloudAuditEntry, CloudConnected, CloudCostReport, CloudDisconnected, CloudHost,
    CloudManager, CloudProvider, CloudStatusReport, CloudSweepItem, CloudSweepReport, CloudTarget,
    CloudTestReport, HostTypeOffer,
};

use crate::api::{CloudApi, Result};
use crate::audit;
use crate::model::{self, Connection, Resource};
use crate::relay::RelayAccess;
use crate::state::Store;
use crate::sweep;

/// The clouds of one cua home.
#[derive(Clone)]
pub struct Clouds {
    apis: Vec<Arc<dyn CloudApi>>,
    store: Arc<Store>,
    home: PathBuf,
    relay: Option<RelayAccess>,
    hosts: Arc<crate::hosts::Hosts>,
    /// Cua's issuer for OIDC links (`None`: from the environment on first
    /// use).
    #[cfg(feature = "federation")]
    pub(crate) issuer: crate::sealed::IssuerSlot,
    /// Who calls, when the host knows better than this process's
    /// environment: `Some(None)` a person, `Some(Some(agent))` an agent.
    pub(crate) caller: Option<Option<String>>,
    /// The account-wide spend of linked clouds ([`crate::spend`]).
    #[cfg(feature = "federation")]
    pub(crate) spend: Arc<crate::spend::IssuerSpend>,
    /// The tokens of sealed connections (Hetzner, Modal).
    #[cfg(feature = "federation")]
    pub(crate) sealed: Arc<crate::sealed::SealedTokens>,
}

impl Clouds {
    /// `apis` with their state under `home`.
    pub fn new(home: &Path, apis: Vec<Arc<dyn CloudApi>>, store: Arc<Store>) -> Self {
        // One issuer slot for links, spend and sealed tokens.
        #[cfg(feature = "federation")]
        let issuer: crate::sealed::IssuerSlot = Arc::default();
        #[cfg(feature = "federation")]
        let spend = Arc::new(crate::spend::IssuerSpend::new(issuer.clone()));
        #[cfg(feature = "federation")]
        store.set_account_spend(spend.clone());
        #[cfg(feature = "federation")]
        let sealed = Arc::new(crate::sealed::SealedTokens::new(
            store.clone(),
            issuer.clone(),
        ));
        #[cfg(feature = "federation")]
        for a in &apis {
            a.attach_secrets(sealed.clone());
        }
        Clouds {
            hosts: Arc::new(crate::hosts::Hosts::new(apis.clone(), store.clone(), None)),
            apis,
            store,
            home: home.to_path_buf(),
            relay: None,
            #[cfg(feature = "federation")]
            issuer,
            #[cfg(feature = "federation")]
            spend,
            #[cfg(feature = "federation")]
            sealed,
            caller: None,
        }
    }

    /// Says who calls instead of reading this process's environment
    /// (`None`: a person; `Some(name)`: that agent). Only a person may
    /// approve what an agent asked for.
    pub fn with_caller(mut self, agent: Option<&str>) -> Self {
        self.caller = Some(agent.map(String::from));
        self
    }

    /// The agent that calls, `None` for a person.
    pub(crate) fn caller_agent(&self) -> Option<String> {
        match &self.caller {
            Some(c) => c.clone(),
            None => cua_sandbox_core::byoc::caller_agent(),
        }
    }

    /// Seals and opens tokens with `sealer` instead of this process's pool
    /// keys ([`crate::sealed::set_pool_sealer`]; tests).
    #[cfg(feature = "federation")]
    pub fn with_sealer(self, sealer: Arc<dyn crate::sealed::PoolSealer>) -> Self {
        self.sealed.set_sealer(sealer);
        self
    }

    /// The tokens of this home's sealed connections.
    #[cfg(feature = "federation")]
    pub fn sealed_tokens(&self) -> &Arc<crate::sealed::SealedTokens> {
        &self.sealed
    }

    /// Links clouds through `issuer` instead of the one the environment
    /// names (tests, self-hosted issuers).
    #[cfg(feature = "federation")]
    pub fn with_issuer(self, issuer: Arc<dyn crate::federation::IssuerClient>) -> Self {
        *self.issuer.lock().expect("issuer") = Some(issuer);
        self
    }

    /// [`Clouds::with_issuer`] on a shared manager (tests, embedders).
    #[cfg(feature = "federation")]
    pub fn set_issuer(&self, issuer: Arc<dyn crate::federation::IssuerClient>) {
        *self.issuer.lock().expect("issuer") = Some(issuer);
    }

    /// Lets the sweeper remove relay machines a failed create left behind,
    /// and elastic hosts join the pool on this relay.
    pub fn with_relay(mut self, relay: RelayAccess) -> Self {
        self.hosts = Arc::new(crate::hosts::Hosts::new(
            self.apis.clone(),
            self.store.clone(),
            Some(relay.clone()),
        ));
        self.relay = Some(relay);
        self
    }

    /// The elastic pool hosts of this home ([`crate::hosts`]).
    pub fn hosts_lifecycle(&self) -> &Arc<crate::hosts::Hosts> {
        &self.hosts
    }

    /// The relay machines recorded as left behind for `provider`: removed
    /// (unless `dry_run`), and their records with them.
    async fn sweep_machines(
        &self,
        provider: &str,
        dry_run: bool,
        actor: &str,
    ) -> Result<Vec<CloudSweepItem>> {
        let t = model::now();
        let mut out = Vec::new();
        for rec in self
            .store
            .resources()
            .map_err(Error::Io)?
            .into_iter()
            .filter(|r| r.provider == provider && r.resource_type == model::RELAY_MACHINE)
        {
            let mut item = CloudSweepItem {
                resource: rec.wire(t),
                action: "delete".into(),
                reason: "a relay computer a failed create left".into(),
            };
            if !dry_run {
                match &self.relay {
                    Some(relay) => match relay.forget(&rec.id).await {
                        Ok(()) => {
                            let _ = self.store.forget(&rec);
                            item.action = "deleted".into();
                            self.store.audit(audit::of_resource(
                                "resource_deleted",
                                &rec,
                                actor,
                                "ok",
                                &item.reason,
                            ));
                        }
                        Err(e) => {
                            item.action = "failed".into();
                            item.reason = e.to_string();
                            self.store.audit(audit::of_resource(
                                "resource_deleted",
                                &rec,
                                actor,
                                "failed",
                                &item.reason,
                            ));
                        }
                    },
                    None => {
                        item.action = "failed".into();
                        item.reason = "no relay account in this process".into();
                    }
                }
            }
            out.push(item);
        }
        Ok(out)
    }

    /// Every Cua resource of `provider` that may still be there: what the
    /// cloud lists by this owner's tags, plus recorded ones it did not
    /// list. A recorded one is dropped only when the cloud, asked in the
    /// record's own region, says it is gone; one the cloud cannot be asked
    /// about (another region than the connection's, or a failed lookup) is
    /// kept and marked unverified, never assumed deleted. Without
    /// `shared`, only what runs and bills: instances and sandboxes.
    async fn live(&self, p: &dyn CloudApi, conn: &Connection, shared: bool) -> Result<Vec<Live>> {
        let owner = self.store.owner_id().map_err(Error::Io)?;
        let listed = p.list_owned(conn, &owner).await?;
        let wanted =
            |r: &Resource| shared || matches!(r.resource_type.as_str(), "instance" | "sandbox");
        let t = model::now();
        let mut out: Vec<Live> = listed
            .iter()
            .filter(|r| r.state != "terminated" && wanted(r))
            .map(|r| Live {
                resource: r.clone(),
                unverified: None,
            })
            .collect();
        for rec in self
            .store
            .resources()
            .map_err(Error::Io)?
            .into_iter()
            .filter(|r| r.provider == p.name())
        {
            if listed.iter().any(|r| sweep::same(&rec, r)) {
                continue;
            }
            if rec.resource_type == model::RELAY_MACHINE {
                if shared && self.relay.is_some() {
                    out.push(Live {
                        resource: rec,
                        unverified: None,
                    });
                }
                continue;
            }
            if !wanted(&rec) {
                continue;
            }
            if rec.id.is_empty() {
                // A create under way (an older one is stale: the sweeper
                // forgets it).
                if t.saturating_sub(rec.created) < sweep::GRACE_SECS {
                    out.push(Live {
                        resource: rec,
                        unverified: None,
                    });
                }
                continue;
            }
            // Asked where the record says it lives (its own region), so a
            // host in another region is seen running, or confirmed gone.
            let unverified = if rec.region.is_empty() && !sweep::in_scope(&rec, conn) {
                Some(format!(
                    "recorded in an unknown region, outside this connection's {}",
                    conn.region
                ))
            } else {
                let mut there = conn.clone();
                if !rec.region.is_empty() && rec.region != conn.region {
                    there.region = rec.region.clone();
                    there.zone = String::new();
                }
                match p.describe(&there, &rec).await {
                    Ok(Some((state, _))) if state != "terminated" => None,
                    Ok(_) => continue,
                    Err(e) => Some(format!(
                        "recorded in {}, could not be checked: {e}",
                        there.region
                    )),
                }
            };
            out.push(Live {
                resource: rec,
                unverified,
            });
        }
        Ok(out)
    }

    /// The sweep behind `sweep`, `auto_sweep` and `disconnect --delete`;
    /// `actor` is what the audit log names.
    async fn sweep_as(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
        actor: &str,
    ) -> Result<Vec<CloudSweepItem>> {
        if let Some(n) = provider {
            self.api(n)?;
        }
        let mut rows = Vec::new();
        // Host records before the sweep: one it deletes also loses its relay
        // machine (or its unused pairing code).
        let hosts_before: Vec<Resource> = if dry_run {
            vec![]
        } else {
            self.hosts.records().unwrap_or_default()
        };
        for p in &self.apis {
            if provider.is_some_and(|n| !n.eq_ignore_ascii_case(p.name())) {
                continue;
            }
            let Some(conn) = self.store.connection(p.name()).map_err(Error::Io)? else {
                if provider.is_some() {
                    return Err(Error::ContribNotConfigured(format!(
                        "{} is not connected (`cua cloud connect {}`)",
                        p.title(),
                        p.name()
                    )));
                }
                continue;
            };
            // One cloud that cannot be reached (signed out, its helper
            // missing) says so in its row; the others are still swept.
            let failed_row = |e: &Error| CloudSweepItem {
                resource: cua_sandbox_core::byoc::CloudResource {
                    provider: p.name().into(),
                    resource_type: "account".into(),
                    region: conn.region.clone(),
                    ..Default::default()
                },
                action: "failed".into(),
                reason: e.to_string(),
            };
            let opts = sweep::SweepOpts {
                dry_run,
                all,
                actor,
                forget_missing: actor != sweep::AUTO_ACTOR,
            };
            match sweep::sweep_provider(p.as_ref(), &conn, &self.store, opts).await {
                Ok(r) => rows.extend(r),
                Err(e) if provider.is_none() => rows.push(failed_row(&e)),
                Err(e) => return Err(e),
            }
            match self.sweep_machines(p.name(), dry_run, actor).await {
                Ok(r) => rows.extend(r),
                Err(e) if provider.is_none() => rows.push(failed_row(&e)),
                Err(e) => return Err(e),
            }
        }
        let after: Vec<Resource> = self.hosts.records().unwrap_or_default();
        for r in hosts_before
            .iter()
            .filter(|r| !after.iter().any(|x| sweep::same(x, r)))
        {
            let name = r
                .extra
                .get("host")
                .cloned()
                .unwrap_or_else(|| r.name.clone());
            self.hosts.drop_relay(r, &name).await;
        }
        Ok(rows)
    }

    /// The state.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub(crate) fn api(&self, name: &str) -> Result<&Arc<dyn CloudApi>> {
        let name = name.trim().to_ascii_lowercase();
        self.apis.iter().find(|p| p.name() == name).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "unknown cloud {name:?} (this build: {})",
                self.apis
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
    }

    fn settings(&self) -> Option<cua_sandbox_core::settings::Settings> {
        cua_sandbox_core::settings::Settings::load_with(self.home.join("config.toml"), |k| {
            std::env::var(k).ok()
        })
        .ok()
    }

    /// Makes `provider` the default location (`default.on`).
    pub(crate) fn make_default(&self, provider: &str) -> Result<()> {
        let key = cua_sandbox_core::settings::key("default.on")
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let mut s = self.settings().ok_or_else(|| {
            Error::InvalidArgument("the cua config file could not be read".into())
        })?;
        s.set(key, provider)
            .map(|_| ())
            .map_err(|e| Error::InvalidArgument(e.to_string()))
    }

    pub(crate) fn default_on(&self) -> String {
        self.settings()
            .and_then(|s| s.default_on().ok())
            .map(|(on, _)| on.to_string())
            .unwrap_or_default()
    }

    pub(crate) fn row(
        &self,
        p: &dyn CloudApi,
        conn: Option<&Connection>,
        default_on: &str,
    ) -> CloudProvider {
        let resolved = conn.cloned().or_else(|| {
            p.resolve(&CloudTarget {
                provider: p.name().into(),
                ..Default::default()
            })
            .ok()
        });
        let c = resolved.clone().unwrap_or_default();
        CloudProvider {
            name: p.name().into(),
            title: p.title().into(),
            tier: p.tier().as_str().into(),
            connected: conn.is_some(),
            default: default_on == p.name(),
            credentials: p.detect(),
            account: c.account.clone(),
            profile: c.profile.clone(),
            region: c.region.clone(),
            zone: c.zone.clone(),
            project: c.project.clone(),
            environment: c.environment.clone(),
            label: c.label(p.title()),
            ttl_hours: conn
                .map(|c| c.ttl_hours)
                .unwrap_or(model::DEFAULT_TTL_HOURS),
            kinds: resolved.map(|c| p.kinds(&c)).unwrap_or_default(),
            auth: conn
                .map(|c| c.auth.as_str().to_string())
                .unwrap_or_default(),
            status: if conn.is_some() {
                cua_sandbox_core::byoc::status::CONNECTED
            } else {
                cua_sandbox_core::byoc::status::NOT_CONNECTED
            }
            .into(),
            budget_usd_month: conn.and_then(|c| c.budget_usd_month),
            max_size: conn
                .and_then(|c| c.max_size)
                .map(|m| m.to_string())
                .unwrap_or_default(),
            spent_month: (conn.map_or_else(
                || {
                    self.store
                        .spent_month(p.name(), model::now())
                        .unwrap_or(0.0)
                },
                |c| self.store.spent_for_status(c, model::now()),
            ) * 100.0)
                .round()
                / 100.0,
            running: self
                .store
                .resources()
                .unwrap_or_default()
                .iter()
                .filter(|r| {
                    r.provider == p.name()
                        && matches!(r.resource_type.as_str(), "instance" | "sandbox")
                        && !matches!(r.state.as_str(), "gone" | "terminated" | "stopped")
                })
                .count() as u32,
            approval: String::new(),
            // An AWS link through the CLI sign-in on this device could link
            // without any key instead (it keeps working until switched).
            suggest_auth: if cfg!(feature = "federation")
                && conn.is_some_and(|c| c.auth.is_cli() && c.provider == "aws")
            {
                "oidc".into()
            } else {
                String::new()
            },
            holders: vec![],
            ..Default::default()
        }
    }
}

#[async_trait]
impl CloudManager for Clouds {
    async fn status(&self, provider: Option<&str>) -> Result<CloudStatusReport> {
        if let Some(n) = provider {
            self.api(n)?;
        }
        let default_on = self.default_on();
        let conns = self.store.connections().map_err(Error::Io)?;
        let t = model::now();
        let providers = self
            .apis
            .iter()
            .filter(|p| provider.is_none_or(|n| n.eq_ignore_ascii_case(p.name())))
            .map(|p| {
                let conn = conns.iter().find(|c| c.provider == p.name());
                self.row(p.as_ref(), conn, &default_on)
            })
            .collect();
        // A pool host's state is what the pool sees (joined, cordoned,
        // offline, booting, gone), not the cloud's word for the machine.
        let hosts = self.hosts.list().await.unwrap_or_default();
        let resources = self
            .store
            .resources()
            .map_err(Error::Io)?
            .iter()
            .filter(|r| provider.is_none_or(|n| r.provider.eq_ignore_ascii_case(n)))
            .map(|r| {
                let mut w = r.wire(t);
                if crate::hosts::is_host_record(r) {
                    let name = r.extra.get("host");
                    if let Some(h) = hosts.iter().find(|h| {
                        h.provider == r.provider
                            && if r.id.is_empty() {
                                Some(&h.name) == name
                            } else {
                                h.id == r.id
                            }
                    }) {
                        w.state = h.state.clone();
                    }
                }
                w
            })
            .collect();
        let mut providers: Vec<CloudProvider> = providers;
        // The price line quotes a machine the linked region sells.
        if let Ok(offers) = self.hosts.offers().await {
            for p in providers.iter_mut().filter(|p| p.connected) {
                let cheapest = offers
                    .iter()
                    .filter(|o| {
                        o.provider == p.name && (p.region.is_empty() || o.region == p.region)
                    })
                    .filter(|o| o.supplies == cua_sandbox_core::byoc::supplies::HOSTS)
                    .filter(|o| o.kinds.iter().any(|k| k == "container"))
                    .min_by(|a, b| {
                        a.usd_per_hour
                            .partial_cmp(&b.usd_per_hour)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some(o) = cheapest {
                    for k in p
                        .kinds
                        .iter_mut()
                        .filter(|k| k.supported && k.kind == "container")
                    {
                        k.machine_type = o.type_name.clone();
                        k.usd_per_hour = o.usd_per_hour;
                    }
                }
            }
        }
        crate::connect::merge_server_status(self, &mut providers, &conns).await;
        crate::alerts::check_all(&self.store, t);
        Ok(CloudStatusReport {
            default_on,
            providers,
            resources,
        })
    }

    async fn test(&self, target: &CloudTarget) -> Result<CloudTestReport> {
        let p = self.api(&target.provider)?;
        p.stage_secret(target);
        let conn = p.resolve(target)?;
        let t = p.test(&conn).await;
        Ok(CloudTestReport {
            provider: p.name().into(),
            ok: t.ok(),
            account: t.account,
            checks: t.checks,
        })
    }

    async fn connect(
        &self,
        target: &CloudTarget,
        make_default: bool,
        ttl_hours: Option<u32>,
    ) -> Result<CloudConnected> {
        let p = self.api(&target.provider)?;
        p.stage_secret(target);
        let mut conn = p.resolve(target)?;
        let tested = p.test(&conn).await;
        if !tested.ok() {
            let failed: Vec<String> = tested
                .checks
                .iter()
                .filter(|c| !c.ok)
                .map(|c| format!("{}: {}", c.name, c.detail))
                .collect();
            return Err(Error::Cloud(format!(
                "{} is not ready, nothing was connected: {}",
                p.title(),
                if failed.is_empty() {
                    "no checks ran".to_string()
                } else {
                    failed.join("; ")
                }
            )));
        }
        conn.account = tested.account.clone();
        conn.ttl_hours = match (p.name(), ttl_hours.unwrap_or(model::DEFAULT_TTL_HOURS)) {
            // A Modal sandbox lives at most 24 hours.
            ("modal", 0) => 24,
            ("modal", h) => h.min(24),
            (_, h) => h,
        };
        conn.connected_at = model::rfc3339(model::now());
        p.persist_secret()?;
        self.store.connect(conn.clone()).map_err(Error::Io)?;
        let mut a = audit::entry("connect", p.name(), "user");
        a.detail = format!("{} ttl {}h", conn.label(p.title()), conn.ttl_hours);
        self.store.audit(a);
        if make_default {
            self.make_default(p.name())?;
        }
        let default_on = self.default_on();
        Ok(CloudConnected {
            provider: self.row(p.as_ref(), Some(&conn), &default_on),
            checks: tested.checks,
            next: None,
            verify: format!("cua cloud status {}", p.name()),
        })
    }

    async fn connect_with(
        &self,
        req: &cua_sandbox_core::byoc::ConnectRequest,
    ) -> Result<CloudConnected> {
        crate::connect::connect_with(self, req).await
    }

    async fn disconnect(&self, provider: &str, delete: bool) -> Result<CloudDisconnected> {
        let p = self.api(provider)?;
        let mut deleted = Vec::new();
        if let Some(conn) = self.store.connection(p.name()).map_err(Error::Io)? {
            if delete {
                // 1. Delete everything Cua created there.
                let rows = self
                    .sweep_as(Some(p.name()), false, true, "disconnect")
                    .await?;
                deleted = rows
                    .iter()
                    .filter(|i| i.action == "deleted")
                    .map(|i| i.resource.clone())
                    .collect();
                // 2. Forget the connection only when the cloud confirms
                // nothing is left (what a failed delete left is named).
                let left = self.live(p.as_ref(), &conn, true).await?;
                if !left.is_empty() {
                    let msg = format!(
                        "{} was NOT disconnected: {} still there after the delete ({}). \
                         The connection is kept so you can retry `cua cloud disconnect {} --delete`.",
                        p.title(),
                        left.len(),
                        describe(&left),
                        p.name()
                    ) + &unverified_hint(&left);
                    let mut a = audit::entry("disconnect", p.name(), "user");
                    a.outcome = "failed".into();
                    a.detail = format!("delete left {}", left.len());
                    self.store.audit(a);
                    return Err(Error::Cloud(msg));
                }
            } else {
                // Never orphan: running Cua resources would keep billing,
                // and a sweep skips a cloud that is not connected.
                let live = self.live(p.as_ref(), &conn, false).await?;
                if !live.is_empty() {
                    let mut a = audit::entry("disconnect", p.name(), "user");
                    a.outcome = "refused".into();
                    a.detail = format!("{} still running", live.len());
                    self.store.audit(a);
                    return Err(Error::Cloud(
                        format!(
                            "{} was NOT disconnected: Cua created {} there that would keep \
                         running (and billing) unseen: {}. Run `cua cloud disconnect {} --delete` \
                         to delete everything Cua created there and disconnect, or \
                         `cua cloud sweep --all --delete`, then disconnect.",
                            p.title(),
                            if live.len() == 1 {
                                "1 resource".to_string()
                            } else {
                                format!("{} resources", live.len())
                            },
                            describe(&live),
                            p.name()
                        ) + &unverified_hint(&live),
                    ));
                }
            }
        }
        if let Some(conn) = self.store.connection(p.name()).map_err(Error::Io)? {
            crate::connect::revoke_on_issuer(self, &conn).await?;
        }
        let disconnected = self.store.disconnect(p.name()).map_err(Error::Io)?;
        p.forget_secret();
        if disconnected {
            let mut a = audit::entry("disconnect", p.name(), "user");
            a.detail = if delete {
                format!("deleted {} first", deleted.len())
            } else {
                String::new()
            };
            self.store.audit(a);
        }
        if self.default_on() == p.name()
            && let Some(mut s) = self.settings()
            && let Ok(key) = cua_sandbox_core::settings::key("default.on")
        {
            let _ = s.unset(key);
        }
        let t = model::now();
        Ok(CloudDisconnected {
            provider: p.name().into(),
            disconnected,
            left: self
                .store
                .resources()
                .map_err(Error::Io)?
                .iter()
                .filter(|r| r.provider == p.name())
                .map(|r| r.wire(t))
                .collect(),
            deleted,
        })
    }

    async fn approve(&self, provider: &str, deny: bool) -> Result<CloudProvider> {
        crate::connect::approve_with(self, provider, deny).await
    }

    async fn sweep(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
    ) -> Result<CloudSweepReport> {
        let rows = self.sweep_as(provider, dry_run, all, "sweep").await?;
        Ok(sweep::report(dry_run, rows))
    }

    async fn host_offers(&self) -> Result<Vec<HostTypeOffer>> {
        self.hosts.offers().await
    }

    async fn hosts(&self) -> Result<Vec<CloudHost>> {
        self.hosts.list().await
    }

    async fn add_host(&self, req: &AddHost) -> Result<CloudHost> {
        self.hosts
            .add(
                req,
                &crate::hosts::HostOptions {
                    env: env_words(),
                    ..Default::default()
                },
            )
            .await
    }

    async fn remove_host(&self, name: &str) -> Result<()> {
        self.hosts.remove(name).await
    }

    async fn cordon_host(&self, name: &str, cordoned: bool) -> Result<()> {
        self.hosts.cordon(name, cordoned).await
    }

    async fn auto_sweep(&self) -> Result<CloudSweepReport> {
        let rows = self.sweep_as(None, false, false, sweep::AUTO_ACTOR).await?;
        // The daemon's regular pass is also when budgets are checked.
        crate::alerts::check_all(&self.store, model::now());
        Ok(sweep::report(false, rows))
    }

    async fn audit(&self, limit: usize) -> Result<Vec<CloudAuditEntry>> {
        self.store.audit_entries(limit).map_err(Error::Io)
    }

    async fn audit_for(
        &self,
        provider: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CloudAuditEntry>> {
        if let Some(n) = provider {
            self.api(n)?;
        }
        // This device's log is read whole when narrowed, so the limit
        // counts that cloud's entries.
        let mut v: Vec<CloudAuditEntry> = self
            .store
            .audit_entries(if provider.is_some() {
                usize::MAX
            } else {
                limit
            })
            .map_err(Error::Io)?
            .into_iter()
            .filter(|e| provider.is_none_or(|p| e.provider.eq_ignore_ascii_case(p)))
            .map(|mut e| {
                e.source = "device".into();
                e
            })
            .collect();
        let conns = self.store.connections().map_err(Error::Io)?;
        v.extend(crate::connect::account_audit(self, &conns, provider, limit).await);
        // Oldest first (RFC 3339 UTC sorts as text), newest last.
        v.sort_by(|a, b| a.ts.cmp(&b.ts));
        let skip = v.len().saturating_sub(limit);
        Ok(v.split_off(skip))
    }

    async fn cost(
        &self,
        provider: Option<&str>,
        by: &str,
        since: Option<&str>,
    ) -> Result<CloudCostReport> {
        use cua_sandbox_core::byoc::cost_by;
        if let Some(n) = provider {
            self.api(n)?;
        }
        let by = by.trim().to_ascii_lowercase();
        let by = if by.is_empty() {
            cost_by::SPACE.to_string()
        } else {
            by
        };
        if !cost_by::ALL.contains(&by.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "by {by:?} is not one of {}",
                cost_by::ALL.join(", ")
            )));
        }
        let now = model::now();
        let since = match since.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => crate::usage::parse_day(s).ok_or_else(|| {
                Error::InvalidArgument(format!("since {s:?} is not a day (YYYY-MM-DD)"))
            })?,
            None => crate::limits::month_start(now),
        };
        if since > now {
            return Err(Error::InvalidArgument(
                "since is in the future: give a day up to today".into(),
            ));
        }
        let runs = self.store.runs(now).map_err(Error::Io)?;
        Ok(crate::usage::report(&runs, provider, &by, since, now))
    }
}

/// The environment a host's cua needs to reach the same relay and
/// environment as this one (`CUA_ENV`, when not production).
fn env_words() -> Vec<String> {
    let env = cua_home::env::CuaEnv::current();
    if env.is_prod() {
        vec![]
    } else {
        vec![format!("CUA_ENV={}", env.name())]
    }
}

/// What to do about resources the connection cannot see.
fn unverified_hint(rs: &[Live]) -> String {
    if rs.iter().all(|l| l.unverified.is_none()) {
        return String::new();
    }
    " An unverified one is recorded but this connection cannot see it (another region, \
     or the cloud did not answer): it was NOT assumed deleted. Connect the cloud again \
     with the region it is in (`cua cloud connect <cloud> --region <region>`) and delete it there."
        .to_string()
}

/// One resource `live` found, and why it could not be confirmed (if it
/// could not).
struct Live {
    resource: Resource,
    unverified: Option<String>,
}

/// "instance i-1, instance i-2 (unverified: recorded in eu-west-1, ...) and 3 more".
fn describe(rs: &[Live]) -> String {
    const SHOWN: usize = 5;
    let mut s = rs
        .iter()
        .take(SHOWN)
        .map(|l| {
            let r = &l.resource;
            let id = if r.id.is_empty() { &r.name } else { &r.id };
            match &l.unverified {
                Some(why) => format!("{} {id} (unverified: {why})", r.resource_type),
                None => format!("{} {id}", r.resource_type),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if rs.len() > SHOWN {
        s.push_str(&format!(" and {} more", rs.len() - SHOWN));
    }
    s
}
