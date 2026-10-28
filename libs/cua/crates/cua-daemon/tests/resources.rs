//! `SpaceService.GetResources` and `WatchResources` over the daemon's
//! loopback listener: this Mac's numbers as one snapshot, then a stream
//! that starts with a snapshot and ends with the client. Temp dirs only.

use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_daemon::server::{DaemonHandle, ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::daemon::v1 as pb;
use pb::watch_resources_response::Event;
use std::time::Duration;

async fn daemon(dir: &std::path::Path) -> (DaemonRuntime, DaemonHandle, DaemonClient) {
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let handle = start(
        runtime.clone(),
        ServerConfig {
            socket_path: None,
            loopback: Some(([127, 0, 0, 1], 0).into()),
            token: "t0k3n".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    let client = DaemonClient::new(DaemonAddress::Url {
        url: handle.loopback_url.clone().unwrap(),
        token: handle.token.clone(),
    })
    .unwrap();
    (runtime, handle, client)
}

#[tokio::test]
async fn the_daemon_serves_this_macs_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let (_runtime, handle, client) = daemon(dir.path()).await;
    let r = client
        .spaces()
        .get_resources(pb::GetResourcesRequest { visible: false })
        .await
        .unwrap()
        .into_inner()
        .snapshot
        .expect("a snapshot");
    let mac = r.this_mac.expect("this Mac");
    assert_eq!(mac.id, "this-mac");
    assert!(mac.online);
    assert!(mac.cpu_count >= 1);
    assert!(mac.memory_total > 0 && mac.memory_used <= mac.memory_total);
    assert!(mac.disk_total > 0 && mac.disk_free <= mac.disk_total);
    assert!((0.0..=100.0).contains(&mac.cpu_percent));
    assert!(r.spaces.is_empty());
    let level = r.warning.expect("a warning level").level;
    assert!(["ok", "low", "critical"].contains(&level.as_str()));
    // The same sample again within a few seconds.
    let again = client
        .spaces()
        .get_resources(pb::GetResourcesRequest { visible: true })
        .await
        .unwrap()
        .into_inner()
        .snapshot
        .unwrap();
    assert_eq!(again.sequence, r.sequence);
    handle.shutdown();
}

#[tokio::test]
async fn a_watch_streams_a_snapshot_first_and_stops_with_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, handle, client) = daemon(dir.path()).await;
    let mut stream = client
        .spaces()
        .watch_resources(pb::WatchResourcesRequest { visible: true })
        .await
        .unwrap()
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(10), stream.message())
        .await
        .expect("an event arrives")
        .unwrap()
        .expect("the stream is open");
    match first.event {
        Some(Event::Snapshot(s)) => {
            assert!(s.this_mac.unwrap().cpu_count >= 1);
            assert_eq!(s.spaces.len(), 0);
        }
        other => panic!("the first event is a snapshot, got {other:?}"),
    }
    drop(stream);
    // The client left: the watch is gone, so a hidden poll still answers
    // and nothing keeps sampling for a window.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runtime.spaces().resources(false).await.is_ok());
    handle.shutdown();
}
