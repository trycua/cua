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
use crate::model::{self, Connection, ProvisionSpec, Resource, Tier};

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
        vec![
            CloudKind {
                image: "linux".into(),
                kind: "container".into(),
                supported: true,
                machine_type: "fake.small".into(),
                usd_per_hour: 0.02,
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

    async fn provision(&self, _: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        let r = {
            let mut a = self.account.lock().unwrap();
            a.calls.push(format!("provision {}", spec.name));
            a.specs.push(spec.clone());
            if a.fail_provision {
                a.fail_provision = false;
                return Err(Error::Cloud("fake: quota exceeded".into()));
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
        _: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        let a = self.account.lock().unwrap();
        Ok(a.resources
            .get(&r.id)
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
        a.resources.remove(&r.id);
        Ok(())
    }

    async fn list_owned(&self, _: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let a = self.account.lock().unwrap();
        Ok(a.resources
            .values()
            .filter(|r| model::is_ours(&r.tags, owner))
            .cloned()
            .collect())
    }
}
