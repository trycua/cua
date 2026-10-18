//! `SpaceService.GetSpaceThumbnail` over the daemon's loopback listener:
//! the daemon's cache answers within `max_age`, falls back to the older
//! image when a fresh capture fails, keeps it across a daemon restart, and
//! says not found when it has nothing. Temp dirs only; no Space answers.

use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_daemon::server::{DaemonHandle, ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::daemon::v1 as pb;
use cua_spaces::thumbnails::Thumbnail;
use std::time::{Duration, UNIX_EPOCH};

const ID: &str = "direct:127.0.0.1:9";

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

fn ask(max_age: Option<Duration>) -> pb::GetSpaceThumbnailRequest {
    pb::GetSpaceThumbnailRequest {
        space: ID.into(),
        max_age: max_age.map(|d| pbjson_types::Duration {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        }),
    }
}

#[tokio::test]
async fn the_daemon_serves_its_shared_thumbnail_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, handle, client) = daemon(dir.path()).await;
    // Nothing cached, nothing answers: an error, not an empty image.
    assert!(
        client
            .spaces()
            .get_space_thumbnail(ask(None))
            .await
            .is_err()
    );

    let captured_at = UNIX_EPOCH + Duration::from_millis(1_800_000_000_000);
    runtime.spaces().thumbnails().put(
        ID,
        Thumbnail {
            image: vec![0xff, 0xd8, 0xff, 0xd9],
            format: "jpeg".into(),
            width: 320,
            height: 200,
            captured_at,
        },
    );
    let r = client
        .spaces()
        .get_space_thumbnail(ask(None))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        (r.image.len(), r.format.as_str(), r.width, r.height),
        (4, "jpeg", 320, 200)
    );
    assert_eq!(r.captured_at.unwrap().seconds, 1_800_000_000);
    // Older than max_age: the capture fails, the older image comes back.
    let stale = client
        .spaces()
        .get_space_thumbnail(ask(Some(Duration::ZERO)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stale.captured_at, r.captured_at);
    handle.shutdown();
    drop((runtime, handle, client));

    // A new daemon on the same home: the cache is still there.
    let (_runtime, handle, client) = daemon(dir.path()).await;
    let again = client
        .spaces()
        .get_space_thumbnail(ask(None))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(again.image, vec![0xff, 0xd8, 0xff, 0xd9]);
    handle.shutdown();
}
