// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`CloudSandboxes`]: one cloud as a sandbox-layer
//! [`cua_sandbox_core::Provider`]. The provider-independent flow of a
//! create, get, delete, stop and start: the relay machine the sandbox joins
//! as, the record before the cloud call, tags, the wait until it is online,
//! and rollback of everything a failed create made. The cloud's own calls
//! are its [`CloudApi`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::provider::{
    ImageMode, PortExposure, ProviderCapabilities, ProviderCreate, ProviderInstance, RunKind,
};
use cua_sandbox_core::{Error, ServiceEndpoint, Status};

use crate::api::{CloudApi, Result};
use crate::bootstrap::{VmBoot, cloud_init};
use crate::model::{self, Connection, ProvisionSpec, Resource, Tier, now};
use crate::relay::RelayAccess;
use crate::state::Store;

/// How long a new cloud machine may take to come online on the relay: a
/// VM's first boot installs Docker and pulls the image.
pub const ONLINE_WAIT: Duration = Duration::from_secs(15 * 60);

/// Instance details (`provider_details`).
pub mod detail {
    /// Where it runs ("AWS · us-west-2").
    pub const PLACE: &str = cua_sandbox_core::byoc::DETAIL_PLACE;
    /// The machine type.
    pub const MACHINE_TYPE: &str = "machine_type";
    /// The estimated cost per hour (US dollars).
    pub const USD_PER_HOUR: &str = "usd_per_hour";
    /// When it expires (RFC 3339), when it does.
    pub const EXPIRES: &str = "expires";
    /// The engine it runs on, when the cloud offers a choice (`vm`).
    pub const RUNTIME: &str = "runtime";
}

/// The image family a reference belongs to (what [`CloudApi::kinds`]
/// prices).
pub fn family_of(reference: &str) -> &'static str {
    let r = reference.to_ascii_lowercase();
    if r.contains("windows") {
        "windows"
    } else if r.contains("omarchy") {
        "omarchy"
    } else if r.contains("macos") {
        "macos"
    } else if r.contains("-slim") {
        "linux-slim"
    } else {
        "linux"
    }
}

/// One cloud as a sandbox provider.
pub struct CloudSandboxes {
    api: Arc<dyn CloudApi>,
    store: Arc<Store>,
    relay: RelayAccess,
}

impl CloudSandboxes {
    /// `api`, recording into `store`, joining its sandboxes to `relay`.
    pub fn new(api: Arc<dyn CloudApi>, store: Arc<Store>, relay: RelayAccess) -> Self {
        CloudSandboxes { api, store, relay }
    }

    /// The cloud's API.
    pub fn api(&self) -> &Arc<dyn CloudApi> {
        &self.api
    }

    fn io(e: std::io::Error) -> Error {
        Error::Io(e)
    }

    fn connection(&self) -> Result<Connection> {
        self.store
            .connection(self.api.name())
            .map_err(Self::io)?
            .ok_or_else(|| {
                Error::ContribNotConfigured(format!(
                    "{} is not connected: run `cua cloud connect {}` first (it checks the \
                     account without creating anything)",
                    self.api.title(),
                    self.api.name()
                ))
            })
    }

    /// What the sandbox's relay machine says about it (no secrets): every
    /// device of the account labels it with it and knows where it can be
    /// deleted permanently.
    fn meta(&self, conn: &Connection, sandbox: &str, owner: &str) -> BTreeMap<String, String> {
        use cua_sandbox_core::byoc::meta;
        let mut m = BTreeMap::from([
            (meta::PROVIDER.to_string(), self.api.name().to_string()),
            (meta::PLACE.to_string(), conn.label(self.api.title())),
            (meta::SANDBOX.to_string(), sandbox.to_string()),
            (meta::OWNER.to_string(), owner.to_string()),
            (meta::DEVICE.to_string(), cua_host::device_name()),
        ]);
        // This home is a host on the relay: other devices ask it to delete.
        if let Some(home) = self.store.home()
            && let Ok(Some(c)) = cua_host::Host::new(&home).config()
            && c.mode == "relay"
            && let Some(id) = c.machine_id.filter(|i| !i.is_empty())
        {
            m.insert(meta::HOST.to_string(), id);
        }
        m.retain(|_, v| !v.is_empty());
        for v in m.values_mut() {
            if v.chars().count() > 256 {
                *v = v.chars().take(256).collect();
            }
        }
        m
    }

    fn record_of(&self, id: &str) -> Result<Option<Resource>> {
        Ok(self
            .store
            .resources()
            .map_err(Self::io)?
            .into_iter()
            .find(|r| {
                r.provider == self.api.name()
                    && matches!(r.resource_type.as_str(), "instance" | "sandbox")
                    && r.id == id
            }))
    }

    fn instance(&self, r: &Resource, status: Status, conn: &Connection) -> ProviderInstance {
        let mut i = ProviderInstance::new(
            r.id.clone(),
            r.sandbox
                .split_once(':')
                .map_or(r.sandbox.clone(), |(_, n)| n.to_string()),
            status,
        );
        if !r.machine.is_empty() {
            i.details
                .insert(DETAIL_RELAY_MACHINE.into(), r.machine.clone());
        }
        i.details
            .insert(detail::PLACE.into(), conn.label(self.api.title()));
        for k in [detail::MACHINE_TYPE, detail::USD_PER_HOUR, detail::RUNTIME] {
            if let Some(v) = r.extra.get(k) {
                i.details.insert(k.into(), v.clone());
            }
        }
        if r.expires > 0 {
            i.details
                .insert(detail::EXPIRES.into(), model::rfc3339(r.expires));
        }
        i
    }

    /// A check the wait runs between polls: fails at once when the machine
    /// is gone or stopped.
    async fn alive(&self, conn: &Connection, r: &Resource) -> Result<()> {
        match self.api.describe(conn, r).await {
            Ok(Some((state, _))) if is_dead(&state) => Err(Error::Cloud(format!(
                "{}: {} {} is {state} before its sandbox came online (see its boot log)",
                self.api.name(),
                r.resource_type,
                r.id
            ))),
            Ok(None) => Err(Error::Cloud(format!(
                "{}: {} {} is gone before its sandbox came online",
                self.api.name(),
                r.resource_type,
                r.id
            ))),
            // A transient API error is not a verdict on the machine.
            _ => Ok(()),
        }
    }

    /// Deletes what a failed create made (best effort; what could not be
    /// deleted stays recorded, for the sweeper).
    async fn rollback(&self, conn: &Connection, owner: &str, made: &[Resource], machine: &str) {
        for r in made {
            if r.id.is_empty() {
                let listed = self.api.list_owned(conn, owner).await.unwrap_or_default();
                if !listed.iter().any(|x| x.name == r.name) {
                    let _ = self.store.forget(r);
                }
                continue;
            }
            match self.api.delete(conn, r, owner).await {
                Ok(()) => {
                    let _ = self.store.forget(r);
                }
                Err(e) => {
                    // Kept, marked: the sweeper deletes it at once (no grace
                    // period for a create that already failed).
                    tracing::warn!(resource = %r.id, "rollback left it for the sweeper: {e}");
                    let mut failed = r.clone();
                    failed.state = "failed".into();
                    let _ = self.store.record(failed);
                }
            }
        }
        if !machine.is_empty() {
            self.forget_machine(machine).await;
        }
    }

    /// Removes a relay machine; when the relay cannot be reached it is
    /// recorded for the sweeper instead of leaking.
    async fn forget_machine(&self, machine: &str) {
        if let Err(e) = self.relay.forget(machine).await {
            tracing::warn!(machine, "relay machine left for the sweeper: {e}");
            let _ = self.store.record(Resource {
                provider: self.api.name().into(),
                id: machine.into(),
                resource_type: model::RELAY_MACHINE.into(),
                name: machine.into(),
                machine: machine.into(),
                created: now(),
                state: "orphaned".into(),
                ..Default::default()
            });
        }
    }
}

/// A state a machine does not come back from on its own.
pub(crate) fn is_dead(state: &str) -> bool {
    matches!(
        state,
        "terminated" | "shutting-down" | "stopping" | "stopped" | "gone" | "deleted"
    )
}

fn status_of(state: &str) -> Status {
    match state {
        "running" => Status::Running,
        // A stopped VM is what `suspend` makes (its disk stays; `resume`
        // starts it): the sandbox layer shows it as suspended, not exited.
        "stopped" | "stopping" | "suspended" => Status::Suspended,
        "pending" | "starting" | "provisioning" => Status::Provisioning,
        other => Status::Unknown(other.to_string()),
    }
}

/// The runtime word a create asked for, checked against what the cloud
/// offers (empty: its default).
pub fn runtime_word(
    api: &dyn CloudApi,
    asked: &cua_sandbox_core::placement::Runtime,
) -> Result<String> {
    use cua_sandbox_core::placement::{Axis, PlacementError, Runtime};
    if *asked == Runtime::Auto {
        return Ok(String::new());
    }
    let offered = api.runtimes();
    if offered.contains(asked) {
        return Ok(asked.as_str().to_string());
    }
    let mut valid = vec!["auto".to_string()];
    valid.extend(offered.iter().map(|r| r.as_str().to_string()));
    Err(PlacementError::new(
        Axis::Runtime,
        asked.as_str(),
        format!(
            "--runtime {asked} does not apply to {} (it offers: {})",
            api.name(),
            valid.join(", ")
        ),
        valid,
    )
    .into())
}

/// The boot disk a family needs, in GiB.
pub fn disk_for(family: &str) -> u32 {
    match family {
        "windows" => 100,
        "omarchy" | "linux-vm" => 50,
        _ => 30,
    }
}

#[async_trait]
impl cua_sandbox_core::Provider for CloudSandboxes {
    fn name(&self) -> &'static str {
        self.api.name()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            // The default engine (`--runtime` picks another where offered).
            runtime: match (self.api.tier(), self.api.runtimes().first()) {
                (_, Some(cua_sandbox_core::placement::Runtime::Gvisor)) => "gvisor",
                (Tier::Vm, _) => "docker-on-vm",
                (Tier::Sandbox, _) => "platform",
            },
            arches: self.api.arches(),
            image_mode: ImageMode::Direct,
            // Every port is reached through the relay (cua-spacesd's own
            // gRPC-Web and WebSocket there).
            ports: PortExposure::Https,
            command: self.api.tier() == Tier::Sandbox,
            env_to_entrypoint: true,
            private_registry: false,
            suspend: self.api.tier() == Tier::Vm,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: &[],
            // No GPU in your cloud (yet): GPU machine types are off.
            gpus: vec![],
        }
    }

    fn check_configured(&self) -> Result<()> {
        self.connection().map(|_| ())
    }

    fn joins_relay(&self) -> bool {
        true
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        let conn = self.connection()?;
        let family = family_of(&spec.image.reference);
        let kinds = self.api.kinds(&conn);
        let offer = kinds.iter().find(|k| k.image == family).ok_or_else(|| {
            Error::UnsupportedImage(format!(
                "{} does not run {family} sandboxes (it runs: {})",
                self.api.title(),
                kinds
                    .iter()
                    .filter(|k| k.supported)
                    .map(|k| k.image.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if !offer.supported {
            return Err(Error::UnsupportedImage(format!(
                "{family} on {}: {}",
                self.api.title(),
                offer.reason
            )));
        }
        let owner = self.store.owner_id().map_err(Self::io)?;
        let created = now();
        let ttl_secs = match spec.ttl {
            Some(t) => t.as_secs(),
            None => u64::from(conn.ttl_hours) * 3600,
        };
        let expires = if ttl_secs == 0 { 0 } else { created + ttl_secs };
        let suffix = format!("{:016x}", rand::random::<u64>());
        let machine = format!("cloud-{suffix}");
        let resource_name = format!("cua-{}-{}", self.api.name(), &suffix[..12]);
        let sandbox = format!("{}:{}", self.api.name(), spec.name);
        // 1. The relay machine the sandbox joins as.
        let join = self
            .relay
            .register_with(&machine, &spec.name, self.meta(&conn, &spec.name, &owner))
            .await?;
        let mut rec = Resource {
            provider: self.api.name().into(),
            resource_type: match self.api.tier() {
                Tier::Vm => "instance".into(),
                Tier::Sandbox => "sandbox".into(),
            },
            name: resource_name.clone(),
            region: conn.region.clone(),
            zone: conn.zone.clone(),
            project: if conn.project.is_empty() {
                conn.environment.clone()
            } else {
                conn.project.clone()
            },
            sandbox: sandbox.clone(),
            machine: machine.clone(),
            created,
            expires,
            tags: model::tags(&owner, &spec.name, &machine, expires),
            state: "pending".into(),
            ..Default::default()
        };
        rec.extra
            .insert(detail::MACHINE_TYPE.into(), offer.machine_type.clone());
        rec.extra.insert(
            detail::USD_PER_HOUR.into(),
            format!("{:.4}", offer.usd_per_hour),
        );
        // 2. Recorded before the cloud call.
        self.store.record(rec.clone()).map_err(Self::io)?;
        let mut provision = ProvisionSpec {
            name: resource_name,
            family: family.into(),
            image: spec.image.pinned_ref.clone(),
            kind: "container".into(),
            machine_type: offer.machine_type.clone(),
            disk_gb: disk_for(family),
            cpus: Some(spec.cpus),
            memory_mb: Some(spec.memory_mb),
            tags: rec.tags.clone(),
            ttl_secs,
            arch: spec.image.arch.clone(),
            runtime: runtime_word(self.api.as_ref(), &spec.runtime)?,
            ..Default::default()
        };
        match self.api.tier() {
            Tier::Vm => {
                provision.user_data = cloud_init(&VmBoot {
                    join: &join,
                    image: &spec.image.pinned_ref,
                    env: &spec.env,
                    expires,
                    ttl_secs,
                    shm_mb: 2048,
                });
            }
            Tier::Sandbox => {
                let mut env = spec.env.clone();
                env.extend(join.guest_env());
                provision.env = env;
                provision.command = spec.command.clone();
            }
        }
        // 3. The cloud call.
        let made = match self.api.provision(&conn, &provision).await {
            Ok(m) => m,
            Err(e) => {
                self.rollback(&conn, &owner, &[rec], &machine).await;
                return Err(e);
            }
        };
        let mut all = Vec::new();
        for (i, r) in made.into_iter().enumerate() {
            let r = if i == 0 {
                rec.id = r.id.clone();
                rec.state = r.state.clone();
                rec.extra.extend(r.extra.clone());
                rec.clone()
            } else {
                r
            };
            self.store.record(r.clone()).map_err(Self::io)?;
            all.push(r);
        }
        // 4. The guest boots and joins the relay.
        let budget = spec.timeout.max(ONLINE_WAIT);
        let joined = self
            .relay
            .wait_online(&machine, budget, || self.alive(&conn, &rec))
            .await;
        if let Err(e) = joined {
            self.rollback(&conn, &owner, &all[..1], &machine).await;
            return Err(e);
        }
        rec.state = "running".into();
        self.store.record(rec.clone()).map_err(Self::io)?;
        Ok(self.instance(&rec, Status::Running, &conn))
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        let conn = self.connection()?;
        let rec = self
            .record_of(id)?
            .ok_or_else(|| Error::NotFound(format!("{}:{id}", self.api.name())))?;
        match self.api.describe(&conn, &rec).await? {
            Some((state, _)) => Ok(self.instance(&rec, status_of(&state), &conn)),
            None => Err(Error::NotFound(format!("{}:{id}", self.api.name()))),
        }
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        let conn = self.connection()?;
        let owner = self.store.owner_id().map_err(Self::io)?;
        let records = self.store.resources().map_err(Self::io)?;
        Ok(self
            .api
            .list_owned(&conn, &owner)
            .await?
            .into_iter()
            .filter(|r| matches!(r.resource_type.as_str(), "instance" | "sandbox"))
            .map(|r| {
                let rec = records
                    .iter()
                    .find(|x| x.provider == r.provider && x.id == r.id)
                    .cloned()
                    .unwrap_or(r.clone());
                self.instance(&rec, status_of(&r.state), &conn)
            })
            .collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let conn = self.connection()?;
        let owner = self.store.owner_id().map_err(Self::io)?;
        let rec = match self.record_of(id)? {
            Some(r) => r,
            None => {
                // Not recorded here: only a resource tagged with this owner
                // is deleted (a create from this home that died before its
                // record was written).
                let listed = self.api.list_owned(&conn, &owner).await?;
                match listed.into_iter().find(|r| r.id == id) {
                    Some(r) => r,
                    None => return Ok(()),
                }
            }
        };
        self.api.delete(&conn, &rec, &owner).await?;
        self.store.forget(&rec).map_err(Self::io)?;
        if !rec.machine.is_empty() {
            self.forget_machine(&rec.machine).await;
        }
        Ok(())
    }

    fn endpoint(&self, instance: &ProviderInstance, _port: u16) -> Result<ServiceEndpoint> {
        let machine = instance
            .details
            .get(DETAIL_RELAY_MACHINE)
            .ok_or_else(|| Error::NotFound(format!("the relay machine of {}", instance.id)))?;
        Ok(ServiceEndpoint {
            url: self.relay.machine_url(machine),
            headers: vec![],
        })
    }

    fn connect_options(
        &self,
        instance: &ProviderInstance,
        _port: u16,
    ) -> Option<Result<cua_spacesd_client::ConnectOptions>> {
        let machine = instance.details.get(DETAIL_RELAY_MACHINE)?;
        Some(self.relay.connect_options(machine))
    }

    // A cloud VM stops (compute billing stops, its disk stays) and starts
    // again; a cloud sandbox platform (Modal) cannot.
    fn power(&self) -> Option<cua_sandbox_core::PowerControl> {
        (self.api.tier() == Tier::Vm).then_some(cua_sandbox_core::PowerControl::Stop)
    }

    async fn suspend(&self, id: &str) -> Result<()> {
        cua_sandbox_core::Provider::stop(self, id).await
    }

    async fn resume(&self, id: &str) -> Result<ProviderInstance> {
        cua_sandbox_core::Provider::start(self, id).await
    }

    async fn stop(&self, id: &str) -> Result<()> {
        let conn = self.connection()?;
        let owner = self.store.owner_id().map_err(Self::io)?;
        let mut rec = self
            .record_of(id)?
            .ok_or_else(|| Error::NotFound(format!("{}:{id}", self.api.name())))?;
        self.api.stop(&conn, &rec, &owner).await?;
        rec.state = "stopped".into();
        self.store.record(rec).map_err(Self::io)
    }

    async fn start(&self, id: &str) -> Result<ProviderInstance> {
        let conn = self.connection()?;
        let owner = self.store.owner_id().map_err(Self::io)?;
        let mut rec = self
            .record_of(id)?
            .ok_or_else(|| Error::NotFound(format!("{}:{id}", self.api.name())))?;
        self.api.start(&conn, &rec, &owner).await?;
        if !rec.machine.is_empty() {
            self.relay
                .wait_online(&rec.machine, ONLINE_WAIT, || self.alive(&conn, &rec))
                .await?;
        }
        rec.state = "running".into();
        self.store.record(rec.clone()).map_err(Self::io)?;
        Ok(self.instance(&rec, Status::Running, &conn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_and_states() {
        assert_eq!(family_of("ghcr.io/trycua/linux:24.04"), "linux");
        assert_eq!(family_of("ghcr.io/trycua/linux:24.04-slim"), "linux-slim");
        assert_eq!(family_of("ghcr.io/trycua/windows:2022"), "windows");
        assert_eq!(status_of("running"), Status::Running);
        assert_eq!(status_of("stopped"), Status::Suspended);
        assert_eq!(status_of("gone"), Status::Unknown("gone".into()));
        assert!(is_dead("terminated") && !is_dead("pending"));
    }
}
