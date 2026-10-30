//! A call on the loopback listener is this machine's user: a caller header
//! the client sets is dropped, so the call is the owner's. Only the 0600
//! socket's peer (the host's cua-spacesd) forwards another caller.
//! Temp dirs and loopback only.

use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::env::v1 as pb;
use cua_proto::env::v1::host_snapshots_service_client::HostSnapshotsServiceClient;
use std::time::Duration;

#[tokio::test]
async fn a_caller_header_on_the_loopback_listener_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let handle = start(
        runtime,
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
    let channel = tonic::transport::Endpoint::from_shared(handle.loopback_url.clone().unwrap())
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = HostSnapshotsServiceClient::new(channel);

    // Exporting a checkpoint is the owner's alone. With no such checkpoint
    // the owner is told so; a forged editor would be told it is not allowed.
    let export = |caller: Option<&str>| {
        let mut req = tonic::Request::new(pb::ExportHostSnapshotRequest {
            name: "nothing".into(),
            ..Default::default()
        });
        req.metadata_mut()
            .insert("authorization", "Bearer t0k3n".parse().unwrap());
        if let Some(c) = caller {
            req.metadata_mut()
                .insert("x-cua-host-caller", c.parse().unwrap());
        }
        req
    };
    let forged = r#"{"account":"someone-else","role":"shared","via":"relay"}"#;
    let status = client
        .export_host_snapshot(export(Some(forged)))
        .await
        .unwrap_err();
    assert_ne!(
        status.code(),
        tonic::Code::PermissionDenied,
        "the forged caller was honored: {status}"
    );
    // The same call with no header is the owner's, so it fails alike.
    let own = client.export_host_snapshot(export(None)).await.unwrap_err();
    assert_eq!(status.code(), own.code());
}
