use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TOKEN: &str = "direct-space-5073-tok";

fn files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .flat_map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() { files(&p) } else { vec![p] }
        })
        .collect()
}

#[tokio::test]
async fn a_media_bridge_to_a_direct_space_sends_its_registered_token() {
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
    let env = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    let space = runtime
        .spaces()
        .add(&env.url(), Some(TOKEN.into()), None)
        .await
        .unwrap();
    assert!(space.id.starts_with("direct:"), "{}", space.id);

    let bridge = client.open_media_bridge(&space.id, None).await.unwrap();
    assert!(bridge.ws_url.starts_with("ws://"), "{}", bridge.ws_url);

    for f in files(dir.path()) {
        if f.file_name().unwrap() != "spaces-credentials.json" {
            let body = std::fs::read(&f).unwrap();
            assert!(
                !String::from_utf8_lossy(&body).contains(TOKEN),
                "{}",
                f.display()
            );
        }
    }
}
