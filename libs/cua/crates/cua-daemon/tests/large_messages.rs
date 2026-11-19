//! A Spaces tool call through the daemon carries more than gRPC's 4 MiB
//! default each way (#4623: `get_desktop_state` of a 6K display returned a
//! 9.5 MB screenshot and failed with "decoded message length too large").
//! Temp dirs and a loopback mock spacesd only.

use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::daemon::v1 as pb;
use cua_spaces::thumbnails::Thumbnail;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::time::{Duration, SystemTime};

#[tokio::test]
async fn a_tool_call_larger_than_4_mib_round_trips_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().to_path_buf()),
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
    let env = MockServer::start(MockAuth::default()).await;
    let space = runtime
        .spaces()
        .add(&env.url(), None, Some("big".into()))
        .await
        .unwrap();

    // More than 4 MiB to the daemon: the tool call's arguments.
    let word = "a".repeat(5 << 20);
    let r = client
        .spaces()
        .call_space_tool(pb::CallSpaceToolRequest {
            name: "space_bash".into(),
            arguments_json: serde_json::json!({
                "space": space.id,
                "command": format!("echo {word}"),
            })
            .to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!r.is_error, "{}", r.content_json);
    assert!(r.content_json.contains("aaaa"));

    // More than 4 MiB back: a full-size image.
    runtime.spaces().thumbnails().put(
        &space.id,
        Thumbnail {
            image: vec![0x5a; 5 << 20],
            format: "png".into(),
            width: 6720,
            height: 3780,
            captured_at: SystemTime::now(),
        },
    );
    let shot = client
        .spaces()
        .get_space_thumbnail(pb::GetSpaceThumbnailRequest {
            space: space.id.clone(),
            max_age: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(shot.image.len(), 5 << 20);
    handle.shutdown();
}
