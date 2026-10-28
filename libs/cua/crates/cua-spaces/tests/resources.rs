//! `Spaces::resources` and `Spaces::watch_resources` against a fake local
//! runtime whose one instance is a `MockServer` as the guest's cua-spacesd:
//! a Space's allocation and guest numbers, this Mac's totals, the summary
//! another computer publishes, and the watcher's snapshot, updates and
//! low-disk event.

use async_trait::async_trait;
use cua_host::testing::FakeRelay;
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::resources::{DiskLevel, ResourcesEvent, THIS_MAC, summary};
use cua_spaces::{SpaceCreate, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const GIB: u64 = 1 << 30;

/// A local runtime whose instances are all one `MockServer` on loopback.
struct FakeRuntime {
    port: u16,
    running: Mutex<BTreeMap<String, bool>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().insert(name.into(), false);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            Some(false) => Ok(InstanceStatus::Stopped),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
}

struct Fixture {
    spaces: Spaces,
    _srv: MockServer,
    _home: tempfile::TempDir,
    _state: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        running: Mutex::new(BTreeMap::new()),
    });
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt)
                .state_dir(state.path())
                .build(),
        )
        .build();
    Fixture {
        spaces,
        _srv: srv,
        _home: home,
        _state: state,
    }
}

async fn create(spaces: &Spaces, name: &str, cpus: u32, memory_mb: u64) {
    spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some(name.into()),
            cpus: Some(cpus),
            memory_mb: Some(memory_mb),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
}

#[tokio::test]
async fn a_local_space_reports_what_it_was_given_and_what_it_uses() {
    let f = fixture().await;
    create(&f.spaces, "aurora", 3, 3072).await;

    let r = f.spaces.resources(false).await.unwrap();
    // This Mac: cores and memory, the volume, and the extra-Space totals.
    let mac = &r.this_mac;
    assert_eq!(mac.id, THIS_MAC);
    assert!(mac.online);
    assert!(mac.cpu_count >= 1 && mac.memory_total > 0);
    assert!(mac.memory_used <= mac.memory_total);
    assert!(mac.disk_total > 0 && mac.disk_free <= mac.disk_total);
    assert_eq!(mac.spaces_count, 1);

    // The Space: allocation from its state file, guest numbers from Metrics.
    assert_eq!(r.spaces.len(), 1);
    let s = &r.spaces[0];
    assert_eq!(s.id, "local:aurora");
    assert_eq!((s.cpu_allocated, s.memory_allocated), (3, 3 * GIB));
    let u = s.usage.expect("the guest answered");
    assert_eq!((u.cpu_count, u.cpu_percent), (4, 12.5));
    assert_eq!(u.load_average_1m, 0.75);
    assert_eq!((u.memory_total, u.memory_limited), (4 * GIB, true));
    assert_eq!((u.disk_total, u.disk_limited), (Some(32 * GIB), true));
    // A fake engine has no disk files to measure: no number, no claim.
    assert_eq!(s.disk_on_host, None);
    // The totals add the running Space up (its power word is empty: it
    // answered, so it runs).
    assert_eq!(mac.spaces_running, 1);
    assert_eq!(mac.spaces_cpu_allocated, 3);
    assert_eq!(mac.spaces_memory_allocated, 3 * GIB);
    assert_eq!(mac.spaces_memory_used, u.memory_used);
    assert_eq!(r.warning, mac.warning);
}

#[tokio::test]
async fn a_poll_within_a_few_seconds_is_the_same_sample() {
    let f = fixture().await;
    let a = f.spaces.resources(false).await.unwrap();
    let b = f.spaces.resources(false).await.unwrap();
    assert_eq!(a.sequence, b.sequence);
    assert_eq!(a.sampled_at_ms, b.sampled_at_ms);
}

#[tokio::test]
async fn nothing_is_reported_about_another_computer_without_its_summary() {
    let f = fixture().await;
    let r = f.spaces.resources(false).await.unwrap();
    assert!(r.computers.is_empty());
}

#[tokio::test]
async fn other_computers_come_from_the_summary_in_their_relay_record() {
    let relay = FakeRelay::start().await;
    let f = fixture().await;
    let summary_value = summary::encode(&cua_spaces::resources::ComputerResources {
        id: "aaaa000000000001".into(),
        online: true,
        sampled_at_ms: 1_760_000_000_000,
        cpu_count: 10,
        cpu_percent: 31.0,
        memory_total: 32 * GIB,
        memory_used: 12 * GIB,
        disk_total: 512 * GIB,
        disk_free: 40 * GIB,
        spaces_count: 2,
        spaces_running: 1,
        warning: cua_spaces::resources::DiskWarning::assess(
            40 * GIB,
            512 * GIB,
            5 * GIB,
            DiskLevel::Ok,
        ),
        ..Default::default()
    });
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    for (id, name, meta) in [
        (
            "aaaa000000000001",
            "Mac mini",
            BTreeMap::from([(summary::META_KEY.to_string(), summary_value)]),
        ),
        ("bbbb000000000002", "Old laptop", BTreeMap::new()),
    ] {
        client
            .register(
                "owner-token",
                &cua_host::relay::RegisterRequest {
                    id: id.into(),
                    name: name.into(),
                    allow: vec![],
                    host: None,
                    meta,
                },
            )
            .await
            .unwrap();
    }
    let spaces = Spaces::builder()
        .home(f._home.path())
        .relay(cua_spaces::RelayAccount::new(
            &relay.url,
            Arc::new(cua_spaces::relay::StaticToken("owner-token".into())),
        ))
        .build();
    spaces.relay_machines().await.unwrap();
    let r = spaces.resources(false).await.unwrap();
    // Only the computer that published a summary is there.
    assert_eq!(r.computers.len(), 1);
    let c = &r.computers[0];
    assert_eq!(c.id, "aaaa000000000001");
    assert_eq!(c.name, "Mac mini");
    assert_eq!((c.cpu_count, c.cpu_percent), (10, 31.0));
    assert_eq!((c.memory_total, c.memory_used), (32 * GIB, 12 * GIB));
    assert_eq!(c.disk_free, 40 * GIB);
    assert_eq!(c.warning.level, DiskLevel::Low);
    assert_eq!((c.spaces_count, c.spaces_running), (2, 1));
}

#[tokio::test]
async fn a_watch_starts_with_a_snapshot_then_sends_only_what_changed() {
    let f = fixture().await;
    create(&f.spaces, "aurora", 2, 2048).await;
    let mut watch = f.spaces.watch_resources(true);
    let first = tokio::time::timeout(Duration::from_secs(10), watch.next())
        .await
        .expect("a snapshot arrives")
        .unwrap();
    let ResourcesEvent::Snapshot(snap) = first else {
        panic!("first event is the snapshot, got {first:?}");
    };
    assert_eq!(snap.spaces.len(), 1);

    // A second Space appears. Nudge the sampler (a window shows/hides) once
    // the last sample is old enough to be redone.
    create(&f.spaces, "borealis", 1, 1024).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    watch.set_visible(false);
    watch.set_visible(true);
    let next = tokio::time::timeout(Duration::from_secs(10), watch.next())
        .await
        .expect("an update arrives")
        .unwrap();
    let ResourcesEvent::Update(update) = next else {
        panic!("then an update, got {next:?}");
    };
    assert!(update.sequence > snap.sequence);
    // Only the new Space (and nothing about the unchanged one).
    assert_eq!(
        update
            .spaces
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["local:borealis"]
    );
    assert!(update.removed.is_empty());
}

#[tokio::test]
async fn dropping_the_last_watch_ends_the_sampling() {
    let f = fixture().await;
    let watch = f.spaces.watch_resources(false);
    drop(watch);
    // Nobody is interested any more: the next poll without `visible` still
    // answers (it samples on demand).
    assert!(f.spaces.resources(false).await.is_ok());
}
