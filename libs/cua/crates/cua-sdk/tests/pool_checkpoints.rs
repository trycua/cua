//! Checkpoints of Spaces on any of your machines through the SDK, embedded
//! and through a `cua daemon` on a Unix socket: `Spaces.create` with image
//! `checkpoint:<name>` is a fork started where the checkpoint is (not an
//! image pull), and `Sandboxes.checkpoint` / `checkpoints` /
//! `delete_checkpoint` look beyond this machine. Hermetic: a private home
//! and no other machines, so nothing starts. Across real machines:
//! `cua-spaces/tests/host_spaces.rs` (routing) and the AWS pool e2e.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, DaemonHandle, ServerConfig},
};
use cua_sdk::{CheckpointSummary, Cua, CuaError, SpaceCreateOptions};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug)]
enum Topology {
    Embedded,
    #[cfg_attr(not(unix), allow(dead_code))]
    DaemonSocket,
}

async fn world(t: Topology) -> (tempfile::TempDir, Option<DaemonHandle>, Arc<Cua>) {
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    match t {
        Topology::Embedded => (dirs, None, Cua::from_runtime(runtime)),
        Topology::DaemonSocket => {
            let cfg = ServerConfig {
                socket_path: Some(dirs.path().join("cua.sock")),
                loopback: None,
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            };
            let h = server::start(runtime, cfg).await.unwrap();
            let cua = Cua::connect(
                Some(h.socket_path.as_ref().unwrap().display().to_string()),
                None,
            )
            .unwrap();
            (dirs, Some(h), cua)
        }
    }
}

async fn suite(t: Topology) {
    let (_dirs, _daemon, cua) = world(t).await;
    let spaces = cua.spaces();

    // No `on`: where the checkpoint is; no machine holds this one.
    let e = spaces
        .create(SpaceCreateOptions {
            image: Some("checkpoint:nowhere".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, CuaError::NotFound(m) if m.contains("no machine of yours holds it")),
        "{t:?} {e:?}"
    );
    // A fork keeps the checkpoint's guest: what cannot apply is refused.
    let e = spaces
        .create(SpaceCreateOptions {
            image: Some("checkpoint:world".into()),
            command: Some(vec!["true".into()]),
            disk_gb: Some(20),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, CuaError::InvalidArgument(m) if m.contains("takes no command, disk size")),
        "{t:?} {e:?}"
    );
    // Only your machines hold checkpoints.
    let e = spaces
        .create(SpaceCreateOptions {
            image: Some("checkpoint:world".into()),
            on: Some("cloud".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(e, CuaError::InvalidArgument(_)), "{t:?} {e:?}");

    let sbx = cua.sandboxes();
    let none: Vec<CheckpointSummary> = sbx.checkpoints().await.unwrap();
    assert!(none.is_empty(), "{t:?} {none:?}");
    let e = sbx.delete_checkpoint("nowhere".into()).await.unwrap_err();
    assert!(matches!(e, CuaError::NotFound(_)), "{t:?} {e:?}");
    let e = sbx
        .checkpoint("no-such-space".into(), "cp".into(), false)
        .await
        .unwrap_err();
    assert!(matches!(e, CuaError::NotFound(_)), "{t:?} {e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded() {
    suite(Topology::Embedded).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_over_unix_socket() {
    suite(Topology::DaemonSocket).await;
}
