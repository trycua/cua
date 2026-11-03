// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A scripted cloud for tests: an in-memory account that may already hold
//! resources Cua did not create (untagged, or another owner's), records
//! every call, and runs a hook when it "boots" a machine (a test makes the
//! guest join a fake relay there).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};

use crate::api::{CloudApi, Result, Target, Tested};
use crate::model::{
    self, Billing, Capabilities, Connection, HostType, ProvisionSpec, Resource, Supplies, Tier,
};

/// Called with each provision (the spec, the created resource).
pub type BootHook = Arc<dyn Fn(&ProvisionSpec, &Resource) + Send + Sync>;

/// The scripted account.
#[derive(Default)]
pub struct FakeAccount {
    /// Every resource in it, by id: Cua's and everyone else's.
    pub resources: BTreeMap<String, Resource>,
    /// Every call, in order (`provision cua-space-...`, `delete i-1`).
    pub calls: Vec<String>,
    /// Make the next provision fail.
    pub fail_provision: bool,
    /// The specs provisioned.
    pub specs: Vec<ProvisionSpec>,
    /// What the console of every machine shows (`None`: nothing).
    pub boot_log: Option<String>,
    /// Make reading the boot log fail.
    pub fail_boot_log: bool,
    /// Make reading the boot log hang (a cloud that never answers).
    pub hang_boot_log: bool,
    /// Make every delete fail (the cloud refuses, or is unreachable).
    pub fail_delete: bool,
    /// Make listing what Cua owns fail (signed out, helper missing).
    pub fail_list: bool,
    /// Refuse a host with more vCPUs than this, as a cloud's core limit
    /// does (`shared core limit exceeded`).
    pub core_limit: Option<u32>,
    /// The machine types it offers instead of the built-in two.
    pub types: Option<Vec<HostType>>,
    /// The type and price `kinds` quotes (default `fake.small`, 0.02).
    pub estimate: Option<(String, f64)>,
}

/// A scripted provider named `name`.
pub struct FakeProvider {
    name: &'static str,
    tier: Tier,
    /// The account.
    pub account: Arc<Mutex<FakeAccount>>,
    hook: Mutex<Option<BootHook>>,
    next: Mutex<u64>,
}

impl FakeProvider {
    /// A provider `name` of `tier` with an empty account.
    pub fn new(name: &'static str, tier: Tier) -> Arc<Self> {
        Arc::new(FakeProvider {
            name,
            tier,
            account: Default::default(),
            hook: Mutex::new(None),
            next: Mutex::new(1),
        })
    }

    /// Runs `hook` for every machine it provisions.
    pub fn on_boot(&self, hook: BootHook) {
        *self.hook.lock().unwrap() = Some(hook);
    }

    /// Puts a resource that already existed in the account.
    pub fn preexisting(&self, r: Resource) {
        self.account
            .lock()
            .unwrap()
            .resources
            .insert(r.id.clone(), r);
    }

    /// The calls so far.
    pub fn calls(&self) -> Vec<String> {
        self.account.lock().unwrap().calls.clone()
    }

    fn guard(&self, r: &Resource, owner: &str, verb: &str) -> Result<()> {
        let a = self.account.lock().unwrap();
        match a.resources.get(&r.id) {
            Some(x) if model::is_ours(&x.tags, owner) => Ok(()),
            Some(_) => Err(Error::Cloud(format!(
                "refusing to {verb} {}: not tagged as this Cua owner's",
                r.id
            ))),
            None => Ok(()),
        }
    }
}

/// Whether `r` lives where `conn` asks (one with no region, anywhere).
fn in_region(r: &Resource, conn: &Connection) -> bool {
    r.region.is_empty() || conn.region.is_empty() || r.region == conn.region
}

#[async_trait]
impl CloudApi for FakeProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn title(&self) -> &'static str {
        "Fake"
    }

    fn tier(&self) -> Tier {
        self.tier
    }

    fn arches(&self) -> Vec<&'static str> {
        vec!["amd64"]
    }

    fn detect(&self) -> CloudCredentials {
        CloudCredentials {
            found: true,
            source: "fake profile".into(),
        }
    }

    fn resolve(&self, t: &Target) -> Result<Connection> {
        Ok(Connection {
            provider: self.name.into(),
            region: t.region.clone().unwrap_or_else(|| "fake-1".into()),
            profile: t.profile.clone().unwrap_or_default(),
            ..Default::default()
        })
    }

    fn kinds(&self, _: &Connection) -> Vec<CloudKind> {
        let (est_type, est_usd) = self
            .account
            .lock()
            .unwrap()
            .estimate
            .clone()
            .unwrap_or(("fake.small".into(), 0.02));
        vec![
            CloudKind {
                image: "linux".into(),
                kind: "container".into(),
                supported: true,
                machine_type: est_type,
                usd_per_hour: est_usd,
                ..Default::default()
            },
            CloudKind {
                image: "macos".into(),
                kind: "vm".into(),
                supported: false,
                reason: "not offered".into(),
                ..Default::default()
            },
        ]
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested {
            account: "fake-account".into(),
            ..Default::default()
        };
        t.check("credentials", true, format!("fake in {}", conn.region));
        self.account.lock().unwrap().calls.push("test".into());
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        if spec.is_host() && self.tier == Tier::Sandbox {
            return Err(Error::InvalidArgument(
                "fake: runs Spaces itself, no hosts".into(),
            ));
        }
        let r = {
            let mut a = self.account.lock().unwrap();
            a.calls.push(format!("provision {}", spec.name));
            a.specs.push(spec.clone());
            if a.fail_provision {
                a.fail_provision = false;
                return Err(Error::Cloud("fake: quota exceeded".into()));
            }
            if spec.is_host()
                && let Some(limit) = a.core_limit
                && let Some(t) = a
                    .types
                    .clone()
                    .unwrap_or_else(|| builtin_types(&conn.region))
                    .iter()
                    .find(|t| t.name == spec.machine_type)
                && t.vcpus > limit
            {
                return Err(Error::Cloud(
                    "fake: create the server: shared core limit exceeded".into(),
                ));
            }
            let mut n = self.next.lock().unwrap();
            let id = format!("fake-{}", *n);
            *n += 1;
            let r = Resource {
                provider: self.name.into(),
                id: id.clone(),
                resource_type: match self.tier {
                    Tier::Vm => "instance".into(),
                    Tier::Sandbox => "sandbox".into(),
                },
                name: spec.name.clone(),
                created: model::now(),
                expires: model::tag_expires(&spec.tags),
                tags: spec.tags.clone(),
                state: "running".into(),
                region: spec.region_or(conn).to_string(),
                ..Default::default()
            };
            a.resources.insert(id, r.clone());
            r
        };
        let hook = self.hook.lock().unwrap().clone();
        if let Some(h) = hook {
            h(spec, &r);
        }
        Ok(vec![r])
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        // Like a regional cloud API: asked in another region, it is not there.
        let a = self.account.lock().unwrap();
        Ok(a.resources
            .get(&r.id)
            .filter(|x| in_region(x, conn))
            .map(|x| (x.state.clone(), x.tags.clone())))
    }

    async fn stop(&self, _: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.guard(r, owner, "stop")?;
        let mut a = self.account.lock().unwrap();
        a.calls.push(format!("stop {}", r.id));
        if let Some(x) = a.resources.get_mut(&r.id) {
            x.state = "stopped".into();
        }
        Ok(())
    }

    async fn start(&self, _: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.guard(r, owner, "start")?;
        let mut a = self.account.lock().unwrap();
        a.calls.push(format!("start {}", r.id));
        if let Some(x) = a.resources.get_mut(&r.id) {
            x.state = "running".into();
        }
        Ok(())
    }

    async fn delete(&self, _: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.guard(r, owner, "delete")?;
        let mut a = self.account.lock().unwrap();
        a.calls.push(format!("delete {}", r.id));
        if a.fail_delete {
            return Err(Error::Cloud(format!("fake: cannot delete {}", r.id)));
        }
        a.resources.remove(&r.id);
        Ok(())
    }

    async fn boot_log(&self, _: &Connection, r: &Resource) -> Result<Option<String>> {
        let (hang, out) = {
            let mut a = self.account.lock().unwrap();
            a.calls.push(format!("boot_log {}", r.id));
            if a.fail_boot_log {
                return Err(Error::Cloud("fake: console unavailable".into()));
            }
            (a.hang_boot_log, a.boot_log.clone())
        };
        if hang {
            std::future::pending::<()>().await;
        }
        Ok(out)
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let a = self.account.lock().unwrap();
        if a.fail_list {
            return Err(Error::Cloud("fake: not signed in".into()));
        }
        Ok(a.resources
            .values()
            .filter(|r| model::is_ours(&r.tags, owner) && in_region(r, conn))
            .cloned()
            .collect())
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        Capabilities {
            supplies: match self.tier {
                Tier::Vm => Supplies::Hosts,
                Tier::Sandbox => Supplies::Spaces,
            },
            billing: Billing {
                granularity_secs: 3600,
                minimum_secs: 3600,
                stopped_bills: true,
            },
            regions: vec![conn.region.clone(), "fake-2".into()],
            max_machines: 5,
            vcpu_quotas: Default::default(),
            self_deletes: false,
        }
    }

    async fn host_types(&self, conn: &Connection) -> Result<Vec<HostType>> {
        if let Some(t) = self.account.lock().unwrap().types.clone() {
            return Ok(t);
        }
        Ok(builtin_types(&conn.region))
    }
}

/// The two machine types a fake cloud offers unless a test sets its own.
fn builtin_types(region: &str) -> Vec<HostType> {
    let regions = vec![region.to_string(), "fake-2".to_string()];
    vec![
        HostType {
            name: "fake.small".into(),
            arch: "amd64".into(),
            vcpus: 2,
            memory_mb: 4096,
            disk_gb: 40,
            kvm: false,
            gpu: String::new(),
            usd_per_hour: 0.02,
            regions: regions.clone(),
            boot_secs: 30,
            quota_pool: "shared".into(),
        },
        HostType {
            name: "fake.large".into(),
            arch: "arm64".into(),
            vcpus: 16,
            memory_mb: 32 * 1024,
            disk_gb: 320,
            // Runs VMs (KVM): forks of a Linux checkpoint fit here.
            kvm: true,
            gpu: String::new(),
            usd_per_hour: 0.07,
            regions,
            boot_secs: 45,
            quota_pool: "shared".into(),
        },
    ]
}
