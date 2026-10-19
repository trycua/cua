//! `Spaces.resources`, `Spaces.watch_resources` and the CPU fields of
//! `Space.usage`, embedded and through a daemon: this Mac's processor,
//! memory and disk as one snapshot, then events a listener hears, ending
//! when the watch is stopped. No host effects.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, DaemonHandle, ServerConfig},
};
use cua_sdk::{Cua, CuaConfig, DiskLevel, ResourcesEvent, ResourcesListener, ResourcesSnapshot};
use cua_spacesd_client::testing::MockServer;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn embedded(dir: &tempfile::TempDir) -> Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

async fn daemon(dir: &tempfile::TempDir) -> (DaemonHandle, Arc<Cua>) {
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().join("cua")),
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "daemon-token".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
    (h, cua)
}

/// Collects what the watch reports.
#[derive(Default)]
struct Heard(Mutex<Vec<ResourcesEvent>>);

impl ResourcesListener for Heard {
    fn on_event(&self, event: ResourcesEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn assert_this_mac(s: &ResourcesSnapshot) {
    let mac = &s.this_mac;
    assert_eq!(mac.id, "this-mac");
    assert!(mac.online && mac.cpu_count >= 1);
    assert!((0.0..=100.0).contains(&mac.cpu_percent));
    assert!(mac.memory_total > 0 && mac.memory_used <= mac.memory_total);
    assert!(mac.disk_total > 0 && mac.disk_free <= mac.disk_total);
    assert_eq!(s.warning, mac.warning);
    assert!(matches!(
        s.warning.level,
        DiskLevel::Ok | DiskLevel::Low | DiskLevel::Critical
    ));
}

async fn first_event(heard: &Arc<Heard>) -> ResourcesEvent {
    for _ in 0..100 {
        if let Some(e) = heard.0.lock().unwrap().first().cloned() {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no event within 10 s");
}

async fn exercise(cua: Arc<Cua>) {
    // One snapshot, polled the way a shell without a stream does.
    let polled = cua.spaces().resources(true).await.unwrap();
    assert_this_mac(&polled);
    assert!(polled.spaces.is_empty() && polled.computers.is_empty());
    let again = cua.spaces().resources(false).await.unwrap();
    assert_eq!(again.sequence, polled.sequence, "the same sample");

    // The same numbers as events: a snapshot first.
    let heard = Arc::new(Heard::default());
    let watch = cua.spaces().watch_resources(true, heard.clone());
    let ResourcesEvent::Snapshot { snapshot } = first_event(&heard).await else {
        panic!("the first event is a snapshot");
    };
    assert_this_mac(&snapshot);
    watch.set_visible(false);
    watch.set_visible(true);
    watch.stop();
    // Nothing is delivered after stop.
    let n = heard.0.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(heard.0.lock().unwrap().len(), n);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_resources_and_watch() {
    let dir = tempfile::tempdir().unwrap();
    exercise(embedded(&dir)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resources_and_watch_through_a_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let (handle, cua) = daemon(&dir).await;
    exercise(cua).await;
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_usage_now_carries_the_processor() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start(Default::default()).await;
    let c = embedded(&dir);
    let info = c
        .spaces()
        .add(server.url(), None, Some("lab".into()))
        .await
        .unwrap();
    let u = c
        .spaces()
        .space(info.id)
        .await
        .unwrap()
        .usage()
        .await
        .unwrap();
    assert_eq!((u.cpu_count, u.cpu_percent), (4, 12.5));
    assert_eq!(u.load_average_1m, 0.75);
    assert_eq!((u.memory_total, u.memory_limited), (4 << 30, true));
    assert_eq!((u.disk_total, u.disk_limited), (32 << 30, true));
}
