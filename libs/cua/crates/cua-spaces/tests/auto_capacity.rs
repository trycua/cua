// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `on="auto"` admits by what Spaces reserve of this machine, never by its
//! load, and a full machine is a typed capacity error, not "not found".
//! Fakes only: a local runtime whose guest port maps to a loopback mock
//! cua-spacesd.

use async_trait::async_trait;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    Reclaimed, RuntimeError, Shortfall,
};
use cua_spaces::{Error, SpaceCreate, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

struct FakeRuntime {
    port: u16,
    running: Mutex<BTreeSet<String>>,
    /// An idle member the runtime gives back when asked to `reclaim`, and
    /// what it was asked for.
    reclaimable: Mutex<Option<String>>,
    /// Idle warm-pool members: held CPU, given back by `reclaim`.
    warm: Mutex<Vec<cua_sandbox_core::WarmHeld>>,
    asked: Mutex<Vec<Shortfall>>,
}

impl FakeRuntime {
    fn endpoints(&self) -> LocalEndpoints {
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        }
    }
    fn instance(&self, name: &str) -> LocalInstance {
        LocalInstance {
            name: name.into(),
            backend: "docker".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        }
    }
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "docker".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(spec.name.clone());
        Ok(self.instance(&spec.name))
    }
    async fn reclaim(&self, need: Shortfall) -> Reclaimed {
        self.asked.lock().unwrap().push(need);
        let held = std::mem::take(&mut *self.warm.lock().unwrap());
        if !held.is_empty() {
            return Reclaimed {
                warm_members: held.len() as u32,
                ended: vec![],
            };
        }
        match self.reclaimable.lock().unwrap().take() {
            Some(name) => {
                self.running.lock().unwrap().remove(&name);
                Reclaimed {
                    warm_members: 1,
                    ended: vec![],
                }
            }
            None => Reclaimed::default(),
        }
    }
    async fn warm_held(&self) -> Vec<cua_sandbox_core::WarmHeld> {
        self.warm.lock().unwrap().clone()
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(name.into());
        Ok(self.instance(name))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(self
            .running
            .lock()
            .unwrap()
            .iter()
            .map(|n| LocalSummary {
                name: n.clone(),
                backend: "docker".into(),
                status: InstanceStatus::Running,
            })
            .collect())
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().contains(name) {
            true => Ok(InstanceStatus::Running),
            false => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(self.endpoints())
    }
}

struct Env {
    spaces: Spaces,
    rt: Arc<FakeRuntime>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    _mock: MockServer,
}

async fn env() -> Env {
    let mock = MockServer::start(MockAuth::default()).await;
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let rt = Arc::new(FakeRuntime {
        port: mock.addr.port(),
        running: Default::default(),
        reclaimable: Default::default(),
        warm: Default::default(),
        asked: Default::default(),
    });
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    Env {
        spaces,
        rt,
        _dirs: (reg, state),
        _mock: mock,
    }
}

fn auto(name: &str, cpus: u32) -> SpaceCreate {
    SpaceCreate {
        on: Some(On::Auto),
        image: Some(IMAGE.into()),
        spacesd: Some(true),
        name: Some(name.into()),
        kind: Kind::Container,
        cpus: Some(cpus),
        memory_mb: Some(256),
        timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    }
}

/// vCPUs of containers this machine takes in all (its CPUs, by the
/// container overcommit).
fn machine_vcpus() -> u32 {
    cua_cluster::sysinfo::cpu_count() * cua_cluster::sysinfo::container_overcommit()
}

#[tokio::test]
async fn an_idle_machine_admits_auto_whatever_its_load() {
    // Nothing reserved: admitted, whatever the load average is.
    let e = env().await;
    e.spaces
        .create(auto("first", 2))
        .await
        .unwrap()
        .ready()
        .unwrap();
}

#[tokio::test]
async fn live_spaces_reserve_and_the_next_create_gets_host_capacity() {
    let e = env().await;
    // One Space that books every CPU of the machine.
    e.spaces
        .create(auto("big", machine_vcpus()))
        .await
        .unwrap()
        .ready()
        .unwrap();
    let err = match e.spaces.create(auto("next", 2)).await {
        Err(err) => err,
        Ok(_) => panic!("a full machine took another Space"),
    };
    assert_eq!(err.tag(), "host_capacity", "{err}");
    let text = err.to_string();
    assert!(
        text.starts_with("this machine has no room for a 2-CPU"),
        "{text}"
    );
    assert!(text.contains("reserved by 1 Space"), "{text}");
    assert!(text.contains("fix: "), "{text}");
    assert!(
        matches!(
            err,
            Error::Sandbox(cua_sandbox_core::Error::Runtime(
                RuntimeError::HostCapacity(_)
            ))
        ),
        "{err:?}"
    );

    // Deleting the big one frees its reservation.
    e.spaces.delete("local:big").await.unwrap();
    e.spaces
        .create(auto("after", 2))
        .await
        .unwrap()
        .ready()
        .unwrap();
}

#[tokio::test]
async fn a_request_no_machine_matches_is_not_found_not_capacity() {
    let e = env().await;
    let mut o = auto("odd", 2);
    o.kind = Kind::MicroVm;
    o.runtime = Runtime::Firecracker;
    let err = match e.spaces.create(o).await {
        Err(err) => err,
        Ok(_) => panic!("a machine without microVMs took a microVM"),
    };
    assert_eq!(err.tag(), "not_found", "{err}");
}

#[tokio::test]
async fn idle_capacity_is_reclaimed_before_refusing() {
    let e = env().await;
    e.spaces
        .create(auto("idle", machine_vcpus()))
        .await
        .unwrap()
        .ready()
        .unwrap();
    // The runtime can give "idle" back (a warm-pool member nobody uses).
    *e.rt.reclaimable.lock().unwrap() = Some("idle".into());
    e.spaces
        .create(auto("wanted", 2))
        .await
        .expect("reclaiming the idle member made room")
        .ready()
        .unwrap();
    let asked = e.rt.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].millicpus > 0 || asked[0].memory_mb > 0,
        "{asked:?}"
    );

    // Nothing reclaimable left and the machine full again: refused, and
    // asked once more first.
    e.spaces.create(auto("fill", machine_vcpus())).await.ok();
    let before = e.rt.asked.lock().unwrap().len();
    let err = match e.spaces.create(auto("late", machine_vcpus())).await {
        Err(err) => err,
        Ok(_) => panic!("a full machine took another Space"),
    };
    assert_eq!(err.tag(), "host_capacity", "{err}");
    assert!(e.rt.asked.lock().unwrap().len() > before);
}

#[tokio::test]
async fn idle_warm_members_count_as_reserved_and_are_given_back() {
    let e = env().await;
    // Two ready members that together hold every CPU of the machine.
    let each = machine_vcpus() / 2;
    *e.rt.warm.lock().unwrap() = vec![
        cua_sandbox_core::WarmHeld {
            container: true,
            cpus: each,
            memory_mb: 256,
        };
        2
    ];
    if each * 2 != machine_vcpus() {
        // An odd CPU count: one more vCPU of the machine stays free.
        return;
    }
    let spaces = &e.spaces;
    spaces
        .create(auto("wanted", 2))
        .await
        .expect("the idle members were given back")
        .ready()
        .unwrap();
    assert!(e.rt.warm.lock().unwrap().is_empty());
    assert_eq!(e.rt.asked.lock().unwrap().len(), 1);
}
