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
    CloudConnected, CloudDisconnected, CloudManager, CloudProvider, CloudStatusReport,
    CloudSweepItem, CloudSweepReport, CloudTarget, CloudTestReport,
};

use crate::api::{CloudApi, Result};
use crate::model::{self, Connection};
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
}

impl Clouds {
    /// `apis` with their state under `home`.
    pub fn new(home: &Path, apis: Vec<Arc<dyn CloudApi>>, store: Arc<Store>) -> Self {
        Clouds {
            apis,
            store,
            home: home.to_path_buf(),
            relay: None,
        }
    }

    /// Lets the sweeper remove relay machines a failed create left behind.
    pub fn with_relay(mut self, relay: RelayAccess) -> Self {
        self.relay = Some(relay);
        self
    }

    /// The relay machines recorded as left behind for `provider`: removed
    /// (unless `dry_run`), and their records with them.
    async fn sweep_machines(&self, provider: &str, dry_run: bool) -> Result<Vec<CloudSweepItem>> {
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
                reason: "a relay machine a failed create left".into(),
            };
            if !dry_run {
                match &self.relay {
                    Some(relay) => match relay.forget(&rec.id).await {
                        Ok(()) => {
                            let _ = self.store.forget(&rec);
                            item.action = "deleted".into();
                        }
                        Err(e) => {
                            item.action = "failed".into();
                            item.reason = e.to_string();
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
        let resources = self
            .store
            .resources()
            .map_err(Error::Io)?
            .iter()
            .filter(|r| provider.is_none_or(|n| r.provider.eq_ignore_ascii_case(n)))
            .map(|r| r.wire(t))
            .collect();
        Ok(CloudStatusReport {
            default_on,
            providers,
            resources,
        })
    }

    async fn test(&self, target: &CloudTarget) -> Result<CloudTestReport> {
        let p = self.api(&target.provider)?;
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
        self.store.connect(conn.clone()).map_err(Error::Io)?;
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

    async fn disconnect(&self, provider: &str) -> Result<CloudDisconnected> {
        let p = self.api(provider)?;
        let disconnected = self.store.disconnect(p.name()).map_err(Error::Io)?;
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
        })
    }

    async fn sweep(
        &self,
        provider: Option<&str>,
        dry_run: bool,
        all: bool,
    ) -> Result<CloudSweepReport> {
        if let Some(n) = provider {
            self.api(n)?;
        }
        let mut rows = Vec::new();
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
            match sweep::sweep_provider(p.as_ref(), &conn, &self.store, dry_run, all).await {
                Ok(r) => rows.extend(r),
                Err(e) if provider.is_none() => rows.push(CloudSweepItem {
                    resource: cua_sandbox_core::byoc::CloudResource {
                        provider: p.name().into(),
                        resource_type: "account".into(),
                        region: conn.region.clone(),
                        ..Default::default()
                    },
                    action: "failed".into(),
                    reason: e.to_string(),
                }),
                Err(e) => return Err(e),
            }
            rows.extend(self.sweep_machines(p.name(), dry_run).await?);
        }
        Ok(sweep::report(dry_run, rows))
    }
}
