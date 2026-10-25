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
    AddHost, CloudAuditEntry, CloudCleanup, CloudConnected, CloudDisconnected, CloudHost,
    CloudManager, CloudProvider, CloudStatusReport, CloudSweepItem, CloudSweepReport,
    CloudTarget, CloudTestReport, HostTypeOffer, signin as wire_signin,
};

use crate::api::{CloudApi, Result};
use crate::audit;
use crate::model::{self, Connection, Resource};
use crate::relay::RelayAccess;
use crate::signin;
use crate::state::{Cleanup, Store};
use crate::sweep;

/// How long one cloud's sweep pass may take before it is given up (nothing
/// is assumed about what it did not finish; the next pass picks up).
pub const SWEEP_PASS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// The clouds of one cua home.
#[derive(Clone)]
pub struct Clouds {
    apis: Vec<Arc<dyn CloudApi>>,
    store: Arc<Store>,
    home: PathBuf,
    relay: Option<RelayAccess>,
    hosts: Arc<crate::hosts::Hosts>,
}

impl Clouds {
    /// `apis` with their state under `home`.
    pub fn new(home: &Path, apis: Vec<Arc<dyn CloudApi>>, store: Arc<Store>) -> Self {
        let apis: Vec<Arc<dyn CloudApi>> = apis.into_iter().map(signin::Guarded::wrap).collect();
        Clouds {
            hosts: Arc::new(crate::hosts::Hosts::new(apis.clone(), store.clone(), None)),
            apis,
            store,
            home: home.to_path_buf(),
            relay: None,
        }
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
        // The credentials first: another account's empty listing must never
        // read as "nothing left".
        sweep::checked_clock(p, conn).await?;
        let listed = p.list_owned(conn, &owner).await?;
        let ambiguous = sweep::ambiguous(&self.store, p.name())?;
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
            .filter(|r| sweep::belongs(r, conn, ambiguous))
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
                    Ok(_) if t.saturating_sub(rec.created) < sweep::CONSISTENCY_SECS => Some(
                        "just created: the cloud may not show it yet".to_string(),
                    ),
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
    /// `actor` is what the audit log names. It sweeps every connected
    /// cloud and every cloud disconnected while it could not be checked
    /// (each with its own connection, never another), under this home's
    /// sweep lease.
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
        let Some(_lease) = self.store.try_sweep_lease().map_err(Error::Io)? else {
            if actor == sweep::AUTO_ACTOR {
                tracing::info!("cloud auto-sweep skipped: another sweep of this home is running");
                return Ok(vec![]);
            }
            return Err(Error::Cloud(
                "Another sweep of this computer's clouds is running. Run this again when it ends."
                    .into(),
            ));
        };
        let mut rows = Vec::new();
        // Host records before the sweep: one it deletes also loses its relay
        // machine (or its unused pairing code).
        let hosts_before: Vec<Resource> = if dry_run {
            vec![]
        } else {
            self.hosts.records().unwrap_or_default()
        };
        let cleanups = self.store.cleanups().map_err(Error::Io)?;
        for p in &self.apis {
            if provider.is_some_and(|n| !n.eq_ignore_ascii_case(p.name())) {
                continue;
            }
            let live = self.store.connection(p.name()).map_err(Error::Io)?;
            let pending: Vec<&Cleanup> = cleanups
                .iter()
                .filter(|c| c.connection.provider == p.name())
                .collect();
            if live.is_none() && pending.is_empty() {
                if provider.is_some() {
                    return Err(Error::ContribNotConfigured(format!(
                        "{} is not connected (`cua cloud connect {}`)",
                        p.title(),
                        p.name()
                    )));
                }
                continue;
            }
            let mut passes: Vec<(Connection, bool)> = Vec::new();
            if let Some(c) = live {
                passes.push((c, false));
            }
            passes.extend(pending.iter().map(|c| (c.connection.clone(), true)));
            for (conn, cleanup) in passes {
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
                    cleanup,
                };
                let pass = tokio::time::timeout(
                    SWEEP_PASS_TIMEOUT,
                    sweep::sweep_provider(p.as_ref(), &conn, &self.store, opts),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(Error::Timeout(format!(
                        "{} did not answer within {} s: nothing more was deleted or assumed gone",
                        p.title(),
                        SWEEP_PASS_TIMEOUT.as_secs()
                    )))
                });
                let ok = match pass {
                    Ok(r) => {
                        rows.extend(r);
                        true
                    }
                    Err(e) if provider.is_none() || cleanup => {
                        rows.push(failed_row(&e));
                        false
                    }
                    Err(e) => return Err(e),
                };
                if cleanup && ok && !dry_run {
                    rows.extend(self.finish_cleanup(p.as_ref(), &conn, actor).await);
                }
            }
            match self.sweep_machines(p.name(), dry_run, actor).await {
                Ok(r) => rows.extend(r),
                Err(e) if provider.is_none() => rows.push(CloudSweepItem {
                    resource: cua_sandbox_core::byoc::CloudResource {
                        provider: p.name().into(),
                        resource_type: "account".into(),
                        ..Default::default()
                    },
                    action: "failed".into(),
                    reason: e.to_string(),
                }),
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

    /// After a cleanup pass of a cloud disconnected while it could not be
    /// checked: when the cloud, asked again, has nothing of Cua's left for
    /// that account and no record names it, the entry is done (and its
    /// saved secret, when no connection uses it, goes). Otherwise it stays.
    async fn finish_cleanup(
        &self,
        p: &dyn CloudApi,
        conn: &Connection,
        actor: &str,
    ) -> Vec<CloudSweepItem> {
        let left = match self.live(p, conn, true).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(provider = p.name(), "cleanup not confirmed: {e}");
                return vec![];
            }
        };
        let ambiguous = sweep::ambiguous(&self.store, p.name()).unwrap_or(true);
        let recorded = self
            .store
            .resources()
            .unwrap_or_default()
            .iter()
            .filter(|r| sweep::belongs(r, conn, ambiguous) && r.resource_type != model::RELAY_MACHINE)
            .count();
        if !left.is_empty() || recorded > 0 {
            return vec![];
        }
        if let Err(e) = self.store.clear_cleanup(p.name(), &conn.account) {
            tracing::warn!(provider = p.name(), "cleanup entry not cleared: {e}");
            return vec![];
        }
        if self.store.connection(p.name()).ok().flatten().is_none() {
            p.forget_secret();
        }
        let mut a = audit::entry("cleanup_done", p.name(), actor);
        a.detail = format!("{}: nothing of Cua's left", conn.label(p.title()));
        self.store.audit(a);
        vec![CloudSweepItem {
            resource: cua_sandbox_core::byoc::CloudResource {
                provider: p.name().into(),
                resource_type: "account".into(),
                region: conn.region.clone(),
                ..Default::default()
            },
            action: "clean".into(),
            reason: format!("{}: nothing of Cua's is left there", p.title()),
        }]
    }

    /// What Cua's records say may still be in `conn`'s account: the line
    /// apps and `cua cloud status` show.
    fn cleanup_of(&self, p: &dyn CloudApi, c: &Cleanup) -> CloudCleanup {
        let ambiguous = sweep::ambiguous(&self.store, p.name()).unwrap_or(true);
        let recs: Vec<Resource> = self
            .store
            .resources()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| {
                sweep::belongs(r, &c.connection, ambiguous) && r.resource_type != model::RELAY_MACHINE
            })
            .collect();
        summarize(p.title(), &c.connection, &recs, c.since, &c.reason)
    }

    /// The state.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    fn api(&self, name: &str) -> Result<&Arc<dyn CloudApi>> {
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

    fn default_on(&self) -> String {
        self.settings()
            .and_then(|s| s.default_on().ok())
            .map(|(on, _)| on.to_string())
            .unwrap_or_default()
    }

    fn row(&self, p: &dyn CloudApi, conn: Option<&Connection>, default_on: &str) -> CloudProvider {
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
        }
    }
}

impl Clouds {
    fn unset_default(&self, provider: &str) {
        if self.default_on() == provider
            && let Some(mut s) = self.settings()
            && let Ok(key) = cua_sandbox_core::settings::key("default.on")
        {
            let _ = s.unset(key);
        }
    }

    /// A disconnect of a cloud that could not be checked (`e`): refused
    /// with what Cua's records say is there unless `force`; with `force`,
    /// the connection becomes a cleanup entry and the records stay.
    async fn unchecked(
        &self,
        p: &dyn CloudApi,
        conn: Connection,
        e: Error,
        force: bool,
    ) -> Result<CloudDisconnected> {
        let raw = e.to_string();
        let sign_in = signin::sign_in_of(p, &conn, &e);
        let reason = sign_in
            .as_ref()
            .map(|s| s.short().to_string())
            .unwrap_or_else(|| "it did not answer".to_string());
        let entry = Cleanup {
            connection: conn.clone(),
            since: model::now(),
            reason: sign_in
                .as_ref()
                .map(|s| s.line.clone())
                .unwrap_or_else(|| wire_signin::split(&raw).0.to_string()),
        };
        let summary = self.cleanup_of(p, &entry);
        let t = model::now();
        let records: Vec<_> = self
            .store
            .resources()
            .map_err(Error::Io)?
            .into_iter()
            .filter(|r| r.provider == p.name())
            .collect();
        if !force {
            let mut a = audit::entry("disconnect", p.name(), "user");
            a.outcome = "refused".into();
            a.detail = format!("could not check: {reason}");
            self.store.audit(a);
            let fix = sign_in
                .as_ref()
                .map(|s| format!("{} ", s.line))
                .unwrap_or_default();
            let line = unchecked_message(p.title(), p.name(), &reason, &summary, t) + "\n" + &fix
                + &format!("To disconnect anyway: `cua cloud disconnect {} --force`.", p.name());
            return Err(Error::Cloud(wire_signin::with_details(
                &line,
                wire_signin::split(&raw).1.unwrap_or(&raw),
            )));
        }
        // Forced with nothing recorded there: nothing to clean up later.
        if summary.resources == 0 {
            let disconnected = self.store.disconnect(p.name()).map_err(Error::Io)?;
            p.forget_secret();
            let mut a = audit::entry("disconnect", p.name(), "user");
            a.outcome = "forced".into();
            a.detail = format!("could not check ({reason}); nothing recorded there");
            self.store.audit(a);
            self.unset_default(p.name());
            return Ok(CloudDisconnected {
                provider: p.name().into(),
                disconnected,
                left: vec![],
                deleted: vec![],
                cleanup: None,
            });
        }
        // Forced: the connection becomes a cleanup entry (its secret stays
        // until the cleanup is done) and the records stay, named for it.
        let disconnected = self
            .store
            .disconnect_to_cleanup(entry)
            .map_err(Error::Io)?;
        let mut a = audit::entry("disconnect", p.name(), "user");
        a.outcome = "forced".into();
        a.detail = format!(
            "could not check ({reason}); kept {} records for a sweep after sign-in",
            summary.resources
        );
        self.store.audit(a);
        self.unset_default(p.name());
        Ok(CloudDisconnected {
            provider: p.name().into(),
            disconnected,
            left: records.iter().map(|r| r.wire(t)).collect(),
            deleted: vec![],
            cleanup: Some(summary),
        })
    }
}

/// "in 6 h", "in 25 min", "in 3 days", "already passed".
pub fn until(expires: u64, now: u64) -> String {
    if expires <= now {
        return "already passed".into();
    }
    let s = expires - now;
    match s {
        0..3600 => format!("in {} min", s.div_ceil(60)),
        3600..172_800 => format!("in {} h", s.div_ceil(3600)),
        _ => format!("in {} days", s.div_ceil(86_400)),
    }
}

/// What Cua's records say may still be in `conn`'s account.
pub fn summarize(
    title: &str,
    conn: &Connection,
    recs: &[Resource],
    since: u64,
    reason: &str,
) -> CloudCleanup {
    let is_space = |r: &Resource| matches!(r.resource_type.as_str(), "instance" | "sandbox");
    let spaces: Vec<&Resource> = recs.iter().filter(|r| is_space(r)).collect();
    let mut kinds: std::collections::BTreeMap<String, u32> = Default::default();
    for r in &spaces {
        let k = match r.extra.get(crate::provider::detail::MACHINE_TYPE) {
            Some(t) if !t.is_empty() => format!("{} {t}", r.resource_type),
            _ => r.resource_type.clone(),
        };
        *kinds.entry(k).or_default() += 1;
    }
    let types = kinds
        .iter()
        .map(|(k, n)| if *n == 1 { k.clone() } else { format!("{k} \u{d7}{n}") })
        .collect::<Vec<_>>()
        .join(", ");
    let latest = spaces.iter().map(|r| r.expires).max().unwrap_or(0);
    let never = spaces.iter().any(|r| r.expires == 0);
    let usd: f64 = spaces
        .iter()
        .filter_map(|r| r.extra.get(crate::provider::detail::USD_PER_HOUR))
        .filter_map(|v| v.parse::<f64>().ok())
        .sum();
    let message = if spaces.is_empty() {
        format!(
            "{title} may still have {} Cua created. Sign in and clean up.",
            plural(recs.len(), "resource")
        )
    } else {
        format!(
            "{title} may still have {} Cua started. Sign in and clean up.",
            plural(spaces.len(), "Space")
        )
    };
    CloudCleanup {
        provider: conn.provider.clone(),
        title: title.into(),
        account: conn.account.clone(),
        profile: conn.profile.clone(),
        region: conn.region.clone(),
        spaces: spaces.len() as u32,
        resources: recs.len() as u32,
        types,
        latest_expires: if latest == 0 {
            String::new()
        } else {
            model::rfc3339(latest)
        },
        never_expires: never,
        usd_per_hour: (usd * 10_000.0).round() / 10_000.0,
        since: model::rfc3339(since),
        reason: reason.into(),
        message,
    }
}

/// "1 Space", "2 Spaces".
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The refusal of a disconnect that could not check the cloud: what Cua
/// started there, what keeps billing until when, and how to finish later.
pub fn unchecked_message(
    title: &str,
    name: &str,
    reason: &str,
    c: &CloudCleanup,
    now: u64,
) -> String {
    let what = if c.resources == 0 {
        "nothing".to_string()
    } else if c.spaces > 0 {
        format!(
            "{} ({}{})",
            plural(c.spaces as usize, "Space"),
            c.types,
            if c.usd_per_hour > 0.0 {
                format!(", about ${:.2}/h", c.usd_per_hour)
            } else {
                String::new()
            }
        )
    } else {
        plural(c.resources as usize, "resource")
    };
    let until_line = if c.never_expires {
        "Anything still running there keeps billing until you delete it (one never expires)".to_string()
    } else if c.latest_expires.is_empty() {
        "Anything still running there keeps billing until it shuts itself down at its expiry".to_string()
    } else {
        let latest = humantime::parse_rfc3339(&c.latest_expires)
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!(
            "Anything still running there keeps billing until it shuts itself down at its expiry (latest: {})",
            until(latest, now)
        )
    };
    format!(
        "{}{title} for Spaces it started ({reason}). Cua's records show {what} there.\n\
         Disconnect anyway? {until_line}. After you sign in, run `cua cloud sweep {name} --delete`.",
        wire_signin::UNCHECKED
    )
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
        let cleanup = self
            .store
            .cleanups()
            .map_err(Error::Io)?
            .iter()
            .filter(|c| provider.is_none_or(|n| c.connection.provider.eq_ignore_ascii_case(n)))
            .filter_map(|c| {
                let p = self.apis.iter().find(|p| p.name() == c.connection.provider)?;
                Some(self.cleanup_of(p.as_ref(), c))
            })
            .collect();
        Ok(CloudStatusReport {
            default_on,
            providers,
            resources,
            cleanup,
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
        // Who the credentials are: every later call checks they still are.
        conn.identity = p
            .identity(&conn)
            .await
            .ok()
            .and_then(|i| i.id)
            .unwrap_or_default();
        conn.ttl_hours = match (p.name(), ttl_hours.unwrap_or(model::DEFAULT_TTL_HOURS)) {
            // A Modal sandbox lives at most 24 hours.
            ("modal", 0) => 24,
            ("modal", h) => h.min(24),
            (_, h) => h,
        };
        conn.connected_at = model::rfc3339(model::now());
        p.persist_secret()?;
        self.store.connect(conn.clone()).map_err(Error::Io)?;
        // The same account again: its Spaces from before a forced
        // disconnect are this connection's again (nothing is deleted;
        // `cua cloud status` lists them, a sweep handles the expired).
        let adopted = self
            .store
            .cleanups()
            .map_err(Error::Io)?
            .into_iter()
            .any(|c| c.same_account(&conn));
        if adopted {
            let _ = self.store.clear_cleanup(p.name(), &conn.account);
            let mut a = audit::entry("cleanup_adopted", p.name(), "user");
            a.detail = format!("{} connected again", conn.label(p.title()));
            self.store.audit(a);
        }
        let mut a = audit::entry("connect", p.name(), "user");
        a.detail = format!("{} ttl {}h", conn.label(p.title()), conn.ttl_hours);
        self.store.audit(a);
        if make_default {
            let key = cua_sandbox_core::settings::key("default.on")
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            let mut s = self.settings().ok_or_else(|| {
                Error::InvalidArgument("the cua config file could not be read".into())
            })?;
            s.set(key, p.name())
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        }
        let default_on = self.default_on();
        Ok(CloudConnected {
            provider: self.row(p.as_ref(), Some(&conn), &default_on),
            checks: tested.checks,
        })
    }

    async fn disconnect(&self, provider: &str, delete: bool) -> Result<CloudDisconnected> {
        self.disconnect_with(provider, delete, false).await
    }

    async fn disconnect_with(
        &self,
        provider: &str,
        delete: bool,
        force: bool,
    ) -> Result<CloudDisconnected> {
        let p = self.api(provider)?;
        let mut deleted = Vec::new();
        if let Some(conn) = self.store.connection(p.name()).map_err(Error::Io)? {
            if delete {
                // 1. Delete everything Cua created there. A cloud that cannot
                // be checked cannot be emptied: `--force` does not apply.
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
                let live = match self.live(p.as_ref(), &conn, false).await {
                    Ok(live) => live,
                    Err(e) => return self.unchecked(p.as_ref(), conn, e, force).await,
                };
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
        self.unset_default(p.name());
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
            cleanup: None,
        })
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
        Ok(sweep::report(false, rows))
    }

    async fn audit(&self, limit: usize) -> Result<Vec<CloudAuditEntry>> {
        self.store.audit_entries(limit).map_err(Error::Io)
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
